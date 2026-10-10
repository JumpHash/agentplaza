// Copyright 2025 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/arch/aarch64/vm.rs (trimmed: the
// snapshot state (`VmState`, `KvmVm::save_state`/`restore_state`), the
// `KVM_CAP_ARM_WRITABLE_IMP_ID_REGS` capability (it only matters for CPU
// templates that rewrite the implementation ID registers), the
// `resource_allocator` restore path and the direct-ioctl helpers are dropped;
// `setup_irqchip` takes the GIC version the caller selected through
// `VmSpec::gic`).

use crate::arch::aarch64::gic::{GICDevice, GICVersion, GicError, create_gic};
use crate::vstate::kvm::Kvm;
use crate::vstate::vm::{VmCommon, VmError};

/// Structure representing the current architecture's understand of what a "virtual machine" is.
#[derive(Debug)]
pub struct KvmVm {
    /// Architecture independent parts of a vm.
    pub common: VmCommon,
    /// On aarch64 we need to keep around the fd obtained by creating the VGIC device.
    irqchip_handle: Option<GICDevice>,
    /// The GIC version the caller selected ([`crate::spawner::VmSpec::gic`]);
    /// `None` is the upstream default, i.e. try GICv3 and fall back to GICv2
    /// (the Raspberry Pi 4/5 path).
    gic_version: Option<GICVersion>,
}

/// Error type for the aarch64-specific [`KvmVm`] setup.
#[derive(Debug, thiserror::Error)]
pub enum KvmVmError {
    /// Error creating the global interrupt controller: {0}
    #[error("Error creating the global interrupt controller: {0}")]
    VmCreateGIC(GicError),
}

impl KvmVm {
    /// Create a new `KvmVm` struct.
    pub fn new(kvm: Kvm) -> Result<KvmVm, VmError> {
        let common = Self::create_common(kvm)?;

        Ok(KvmVm {
            common,
            irqchip_handle: None,
            gic_version: None,
        })
    }

    /// Pre-vCPU creation setup.
    pub fn arch_pre_create_vcpus(&mut self, _: u8) -> Result<(), KvmVmError> {
        Ok(())
    }

    /// Post-vCPU creation setup.
    pub fn arch_post_create_vcpus(&mut self, nr_vcpus: u8) -> Result<(), KvmVmError> {
        // On aarch64, the vCPUs need to be created (i.e call KVM_CREATE_VCPU) before setting up the
        // IRQ chip because the `KVM_CREATE_VCPU` ioctl will return error if the IRQCHIP
        // was already initialized.
        // Search for `kvm_arch_vcpu_create` in arch/arm/kvm/arm.c.
        self.setup_irqchip(nr_vcpus)
    }

    /// Selects the GIC version to create.
    ///
    /// Must be called before the vCPUs are created (the GIC itself is created
    /// right after them, in [`KvmVm::arch_post_create_vcpus`]). `None` keeps
    /// the upstream default: GICv3, falling back to GICv2.
    pub fn set_gic_version(&mut self, version: Option<GICVersion>) {
        self.gic_version = version;
    }

    /// Creates the GIC (Global Interrupt Controller).
    pub fn setup_irqchip(&mut self, vcpu_count: u8) -> Result<(), KvmVmError> {
        let version = self.gic_version.take();
        self.irqchip_handle = Some(
            create_gic(self.fd(), vcpu_count.into(), version).map_err(KvmVmError::VmCreateGIC)?,
        );
        Ok(())
    }

    /// Gets a reference to the irqchip of the VM.
    ///
    /// # Panics
    ///
    /// Only if called before [`KvmVm::arch_post_create_vcpus`] ran, which the
    /// only caller (`configure_system_for_boot`) cannot do: it is called with
    /// the vCPUs `KvmVm::create_vcpus` created.
    pub fn get_irqchip(&self) -> &GICDevice {
        self.irqchip_handle.as_ref().expect("IRQ chip not set")
    }
}
