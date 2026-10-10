// Copyright 2019 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/arch/aarch64/gic/mod.rs (trimmed: the
// GIC device's role in VM snapshots — `KvmVm::save_state`/`restore_state` —
// and the ITS register state are dropped, so `save_device`/`restore_device`
// below cover the distributor, redistributor and CPU interface register
// models only; the ITS *device* is still created for GICv3, because the guest
// finds it through the FDT and would fault on its registers otherwise).
//
// The GIC version selection in `create_gic` is upstream's, verbatim: `None`
// tries GICv3 and falls back to GICv2, which is what makes a Raspberry Pi 4/5
// (GICv2 only) boot with the default configuration.

mod gicv2;
mod gicv3;
mod regs;

use gicv2::GICv2;
use gicv3::GICv3;
use kvm_ioctls::{DeviceFd, VmFd};
pub use regs::GicState;

use super::fdt::GicFdt;
use super::layout;

/// Represent a V2 or V3 GIC device
#[derive(Debug)]
pub struct GIC {
    /// The file descriptor for the KVM device
    fd: DeviceFd,

    /// GIC device properties, to be used for setting up the fdt entry
    properties: [u64; 4],

    /// MSI properties of the GIC device
    msi_properties: Option<[u64; 2]>,

    /// Number of CPUs handled by the device
    vcpu_count: u64,

    /// ITS device
    its_device: Option<DeviceFd>,
}
impl GIC {
    /// Returns the file descriptor of the GIC device
    pub fn device_fd(&self) -> &DeviceFd {
        &self.fd
    }

    /// Returns an array with GIC device properties
    pub fn device_properties(&self) -> &[u64] {
        &self.properties
    }

    /// Returns the number of vCPUs this GIC handles
    pub fn vcpu_count(&self) -> u64 {
        self.vcpu_count
    }
}

/// Errors thrown while setting up the GIC.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GicError {
    /// Error while calling KVM ioctl for setting up the global interrupt controller: {0}
    #[error("Error while calling KVM ioctl for setting up the global interrupt controller: {0}")]
    CreateGIC(kvm_ioctls::Error),
    /// Error while setting or getting device attributes for the GIC: {0}, {1}, {2}
    #[error("Error while setting or getting device attributes for the GIC: {0}, {1}, {2}")]
    DeviceAttribute(kvm_ioctls::Error, bool, u32),
    /// The number of vCPUs in the GicState doesn't match the number of vCPUs on the system.
    #[error("The number of vCPUs in the GicState doesn't match the number of vCPUs on the system.")]
    InconsistentVcpuCount,
    /// The VgicSysRegsState is invalid.
    #[error("The VgicSysRegsState is invalid.")]
    InvalidVgicSysRegState,
}

/// List of implemented GICs.
#[derive(Debug)]
pub enum GICVersion {
    /// Legacy version.
    GICV2,
    /// GICV3 without ITS.
    GICV3,
}

/// Trait for GIC devices.
#[derive(Debug)]
pub enum GICDevice {
    /// Legacy version.
    V2(GICv2),
    /// GICV3 without ITS.
    V3(GICv3),
}
impl GICDevice {
    /// Returns the file descriptor of the GIC device
    pub fn device_fd(&self) -> &DeviceFd {
        match self {
            Self::V2(x) => x.device_fd(),
            Self::V3(x) => x.device_fd(),
        }
    }

    /// Returns the file descriptor of the ITS device, if any
    pub fn its_fd(&self) -> Option<&DeviceFd> {
        match self {
            Self::V2(_) => None,
            Self::V3(x) => x.its_device.as_ref(),
        }
    }

    /// Returns an array with GIC device properties
    pub fn device_properties(&self) -> &[u64] {
        match self {
            Self::V2(x) => x.device_properties(),
            Self::V3(x) => x.device_properties(),
        }
    }

    /// Returns an array with MSI properties if GIC supports it
    pub fn msi_properties(&self) -> Option<&[u64; 2]> {
        match self {
            Self::V2(x) => x.msi_properties.as_ref(),
            Self::V3(x) => x.msi_properties.as_ref(),
        }
    }

    /// Returns the number of vCPUs this GIC handles
    pub fn vcpu_count(&self) -> u64 {
        match self {
            Self::V2(x) => x.vcpu_count(),
            Self::V3(x) => x.vcpu_count(),
        }
    }

    /// Returns the fdt compatibility property of the device
    pub fn fdt_compatibility(&self) -> &str {
        match self {
            Self::V2(x) => x.fdt_compatibility(),
            Self::V3(x) => x.fdt_compatibility(),
        }
    }

    /// Returns the maint_irq fdt property of the device
    pub fn fdt_maint_irq(&self) -> u32 {
        match self {
            Self::V2(x) => x.fdt_maint_irq(),
            Self::V3(x) => x.fdt_maint_irq(),
        }
    }

    /// Returns the GIC version of the device
    pub fn version(&self) -> u32 {
        match self {
            Self::V2(_) => GICv2::VERSION,
            Self::V3(_) => GICv3::VERSION,
        }
    }

    /// Setup the device-specific attributes
    pub fn init_device_attributes(gic_device: &Self) -> Result<(), GicError> {
        match gic_device {
            Self::V2(x) => GICv2::init_device_attributes(x),
            Self::V3(x) => GICv3::init_device_attributes(x),
        }
    }

    /// Method to save the state of the GIC device.
    pub fn save_device(&self, mpidrs: &[u64]) -> Result<GicState, GicError> {
        match self {
            Self::V2(x) => x.save_device(mpidrs),
            Self::V3(x) => x.save_device(mpidrs),
        }
    }

    /// Method to restore the state of the GIC device.
    pub fn restore_device(&self, mpidrs: &[u64], state: &GicState) -> Result<(), GicError> {
        match self {
            Self::V2(x) => x.restore_device(mpidrs, state),
            Self::V3(x) => x.restore_device(mpidrs, state),
        }
    }
}

/// Describes the interrupt controller to the guest's flattened device tree.
///
/// This is how the FDT generator stays free of KVM types: it consumes the
/// trait, the KVM backend implements it on [`GICDevice`].
impl GicFdt for GICDevice {
    fn compatible(&self) -> &str {
        self.fdt_compatibility()
    }

    fn device_properties(&self) -> &[u64] {
        self.device_properties()
    }

    fn fdt_maint_irq(&self) -> u32 {
        self.fdt_maint_irq()
    }

    fn msi_properties(&self) -> Option<&[u64]> {
        self.msi_properties().map(<[u64; 2]>::as_slice)
    }
}

/// Create a GIC device.
///
/// If "version" parameter is "None" the function will try to create by default a GICv3 device.
/// If that fails it will try to fall-back to a GICv2 device.
/// If version is Some the function will try to create a device of exactly the specified version.
pub fn create_gic(
    vm: &VmFd,
    vcpu_count: u64,
    version: Option<GICVersion>,
) -> Result<GICDevice, GicError> {
    match version {
        Some(GICVersion::GICV2) => GICv2::create(vm, vcpu_count).map(GICDevice::V2),
        Some(GICVersion::GICV3) => GICv3::create(vm, vcpu_count).map(GICDevice::V3),
        None => GICv3::create(vm, vcpu_count)
            .map(GICDevice::V3)
            .or_else(|_| GICv2::create(vm, vcpu_count).map(GICDevice::V2)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both controllers live below the 32-bit MMIO window start, one after the
    /// other: the guest's DRAM (which starts at 2 GiB) and the serial console
    /// window (which starts at `MMIO32_MEM_START`) must not overlap them.
    #[test]
    fn gic_windows_do_not_overlap_dram_or_the_serial_window() {
        let windows = [
            // (base, size) of each controller's register windows, in descending
            // address order.
            (
                gicv3::GicAddresses::dist_addr(),
                gicv3::GicAddresses::dist_size(),
            ),
            (
                gicv3::GicAddresses::redists_addr(4),
                gicv3::GicAddresses::redists_size(4),
            ),
            (
                gicv3::GicAddresses::msi_addr(4),
                gicv3::GicAddresses::msi_size(),
            ),
            (
                gicv2::GicAddresses::dist_addr(),
                gicv2::GicAddresses::dist_size(),
            ),
            (
                gicv2::GicAddresses::cpu_addr(),
                gicv2::GicAddresses::cpu_size(),
            ),
        ];

        for (base, size) in windows {
            assert!(size > 0);
            assert!(
                base + size <= layout::MMIO32_MEM_START,
                "a controller window at {base:#x} would overlap the serial console"
            );
            assert!(
                base + size <= layout::DRAM_MEM_START,
                "a controller window at {base:#x} would overlap guest DRAM"
            );
        }
    }
}
