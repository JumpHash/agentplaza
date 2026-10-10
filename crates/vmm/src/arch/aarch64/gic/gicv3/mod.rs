// Copyright 2019 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/arch/aarch64/gic/gicv3/mod.rs (trimmed:
// the ITS register save/restore of snapshots is dropped — the ITS *device* is
// still created, since the guest finds it through the FDT; the address
// arithmetic is spelled through [`GicAddresses`] so it can be unit-tested
// without a KVM device).

pub mod regs;

use kvm_ioctls::{DeviceFd, VmFd};

use crate::arch::aarch64::gic::{GicError, GicState};

/// The MMIO windows of the GICv3 controller.
///
/// Upstream keeps these as private associated functions of [`GICv3`]; they are
/// split out here (with the same values) so the address arithmetic — which
/// must place the distributor, the redistributors and the ITS below the device
/// MMIO window and outside DRAM — can be tested on the host, without a
/// `/dev/kvm` to create the device with.
pub(super) struct GicAddresses;

impl GicAddresses {
    /// Get the address of the GIC distributor.
    pub(super) fn dist_addr() -> u64 {
        super::layout::MMIO32_MEM_START - GICv3::KVM_VGIC_V3_DIST_SIZE
    }

    /// Get the size of the GIC distributor.
    pub(super) fn dist_size() -> u64 {
        GICv3::KVM_VGIC_V3_DIST_SIZE
    }

    /// Get the address of the GIC redistributors.
    pub(super) fn redists_addr(vcpu_count: u64) -> u64 {
        Self::dist_addr() - Self::redists_size(vcpu_count)
    }

    /// Get the size of the GIC redistributors.
    pub(super) fn redists_size(vcpu_count: u64) -> u64 {
        vcpu_count * GICv3::KVM_VGIC_V3_REDIST_SIZE
    }

    /// Get the MSI address
    pub(super) fn msi_addr(vcpu_count: u64) -> u64 {
        Self::redists_addr(vcpu_count) - GICv3::GIC_V3_ITS_SIZE
    }

    /// Get the MSI size
    pub(super) fn msi_size() -> u64 {
        GICv3::GIC_V3_ITS_SIZE
    }
}

#[derive(Debug)]
pub struct GICv3(super::GIC);

impl std::ops::Deref for GICv3 {
    type Target = super::GIC;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for GICv3 {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl GICv3 {
    // Unfortunately bindgen omits defines that are based on other defines.
    // See arch/arm64/include/uapi/asm/kvm.h file from the linux kernel.
    const SZ_64K: u64 = 0x0001_0000;
    const KVM_VGIC_V3_DIST_SIZE: u64 = GICv3::SZ_64K;
    const KVM_VGIC_V3_REDIST_SIZE: u64 = (2 * GICv3::SZ_64K);
    const GIC_V3_ITS_SIZE: u64 = 0x2_0000;

    // Device trees specific constants
    const ARCH_GIC_V3_MAINT_IRQ: u32 = 9;

    pub const VERSION: u32 = kvm_bindings::kvm_device_type_KVM_DEV_TYPE_ARM_VGIC_V3;

    pub fn fdt_compatibility(&self) -> &str {
        "arm,gic-v3"
    }

    pub fn fdt_maint_irq(&self) -> u32 {
        GICv3::ARCH_GIC_V3_MAINT_IRQ
    }

    /// Create the GIC device object
    pub fn create_device(vm: &VmFd, vcpu_count: u64) -> Result<Self, GicError> {
        // Create the GIC device
        let mut gic_device = kvm_bindings::kvm_create_device {
            type_: Self::VERSION,
            fd: 0,
            flags: 0,
        };

        let gic_fd = vm
            .create_device(&mut gic_device)
            .map_err(GicError::CreateGIC)?;

        Ok(GICv3(super::GIC {
            fd: gic_fd,
            properties: [
                GicAddresses::dist_addr(),
                GicAddresses::dist_size(),
                GicAddresses::redists_addr(vcpu_count),
                GicAddresses::redists_size(vcpu_count),
            ],
            msi_properties: Some([GicAddresses::msi_addr(vcpu_count), GicAddresses::msi_size()]),
            vcpu_count,
            its_device: None,
        }))
    }

    /// Save the state of the distributor, redistributor and CPU interface
    /// registers (the ITS tables live in guest memory and are not part of this
    /// port's snapshot model, which does not exist).
    pub fn save_device(&self, mpidrs: &[u64]) -> Result<GicState, GicError> {
        regs::save_state(&self.fd, mpidrs)
    }

    /// Restore the state of the distributor, redistributor and CPU interface
    /// registers.
    pub fn restore_device(&self, mpidrs: &[u64], state: &GicState) -> Result<(), GicError> {
        regs::restore_state(&self.fd, mpidrs, state)
    }

    pub fn init_device_attributes(gic_device: &Self) -> Result<(), GicError> {
        // Setting up the distributor attribute.
        // We are placing the GIC below 1GB so we need to subtract the size of the distributor.
        Self::set_device_attribute(
            gic_device.device_fd(),
            kvm_bindings::KVM_DEV_ARM_VGIC_GRP_ADDR,
            u64::from(kvm_bindings::KVM_VGIC_V3_ADDR_TYPE_DIST),
            &GicAddresses::dist_addr() as *const u64 as u64,
            0,
        )?;

        // Setting up the redistributors' attribute.
        // We are calculating here the start of the redistributors address. We have one per CPU.
        Self::set_device_attribute(
            gic_device.device_fd(),
            kvm_bindings::KVM_DEV_ARM_VGIC_GRP_ADDR,
            u64::from(kvm_bindings::KVM_VGIC_V3_ADDR_TYPE_REDIST),
            &GicAddresses::redists_addr(gic_device.vcpu_count()) as *const u64 as u64,
            0,
        )?;

        Ok(())
    }

    fn init_its(vm: &VmFd, gic_device: &mut Self) -> Result<(), GicError> {
        // ITS part attributes
        let mut its_device = kvm_bindings::kvm_create_device {
            type_: kvm_bindings::kvm_device_type_KVM_DEV_TYPE_ARM_VGIC_ITS,
            fd: 0,
            flags: 0,
        };

        let its_fd = vm
            .create_device(&mut its_device)
            .map_err(GicError::CreateGIC)?;

        // Setting up the ITS attributes
        Self::set_device_attribute(
            &its_fd,
            kvm_bindings::KVM_DEV_ARM_VGIC_GRP_ADDR,
            u64::from(kvm_bindings::KVM_VGIC_ITS_ADDR_TYPE),
            &GicAddresses::msi_addr(gic_device.vcpu_count()) as *const u64 as u64,
            0,
        )?;

        Self::set_device_attribute(
            &its_fd,
            kvm_bindings::KVM_DEV_ARM_VGIC_GRP_CTRL,
            u64::from(kvm_bindings::KVM_DEV_ARM_VGIC_CTRL_INIT),
            0,
            0,
        )?;

        gic_device.its_device = Some(its_fd);
        Ok(())
    }

    /// Method to initialize the GIC device
    pub fn create(vm: &VmFd, vcpu_count: u64) -> Result<Self, GicError> {
        let mut device = Self::create_device(vm, vcpu_count)?;

        Self::init_device_attributes(&device)?;
        Self::init_its(vm, &mut device)?;

        Self::finalize_device(&device)?;

        Ok(device)
    }

    /// Finalize the setup of a GIC device
    pub fn finalize_device(gic_device: &Self) -> Result<(), GicError> {
        // On arm there are 3 types of interrupts: SGI (0-15), PPI (16-31), SPI (32-1020).
        // SPIs are used to signal interrupts from various peripherals accessible across
        // the whole system so these are the ones that we increment when adding a new virtio device.
        // KVM_DEV_ARM_VGIC_GRP_NR_IRQS sets the number of interrupts (SGI, PPI, and SPI).
        // Consequently, we need to add 32 to the number of SPIs ("legacy GSI").
        let nr_irqs: u32 = super::layout::GSI_LEGACY_NUM + super::layout::SPI_START;
        let nr_irqs_ptr = &nr_irqs as *const u32;
        Self::set_device_attribute(
            gic_device.device_fd(),
            kvm_bindings::KVM_DEV_ARM_VGIC_GRP_NR_IRQS,
            0,
            nr_irqs_ptr as u64,
            0,
        )?;

        // Finalize the GIC.
        // See https://code.woboq.org/linux/linux/virt/kvm/arm/vgic/vgic-kvm-device.c.html#211.
        Self::set_device_attribute(
            gic_device.device_fd(),
            kvm_bindings::KVM_DEV_ARM_VGIC_GRP_CTRL,
            u64::from(kvm_bindings::KVM_DEV_ARM_VGIC_CTRL_INIT),
            0,
            0,
        )?;

        Ok(())
    }

    /// Set a GIC device attribute
    pub fn set_device_attribute(
        fd: &DeviceFd,
        group: u32,
        attr: u64,
        addr: u64,
        flags: u32,
    ) -> Result<(), GicError> {
        let attr = kvm_bindings::kvm_device_attr {
            flags,
            group,
            attr,
            addr,
        };
        fd.set_device_attr(&attr)
            .map_err(|err| GicError::DeviceAttribute(err, true, group))?;

        Ok(())
    }
}

/// Function that flushes RDIST pending tables into guest RAM.
///
/// The tables get flushed to guest RAM whenever the VM gets stopped.
fn save_pending_tables(gic_device: &DeviceFd) -> Result<(), GicError> {
    let init_gic_attr = kvm_bindings::kvm_device_attr {
        group: kvm_bindings::KVM_DEV_ARM_VGIC_GRP_CTRL,
        attr: u64::from(kvm_bindings::KVM_DEV_ARM_VGIC_SAVE_PENDING_TABLES),
        addr: 0,
        flags: 0,
    };
    gic_device.set_device_attr(&init_gic_attr).map_err(|err| {
        GicError::DeviceAttribute(err, true, kvm_bindings::KVM_DEV_ARM_VGIC_GRP_CTRL)
    })
}
