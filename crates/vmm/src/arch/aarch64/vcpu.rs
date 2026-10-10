// Copyright 2023 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/arch/aarch64/vcpu.rs (trimmed: the
// CPU-template machinery (`VcpuConfig`, `CpuConfiguration`,
// `RegisterValueFilter`, `VcpuFeatures`), SVE finalization, the snapshot state
// (`VcpuState`, the full register dump and its restore path, `get_mpstate` /
// `set_mpstate`), the pvtime (steal-time) support, the metrics/logger crates
// and the `Peripherals` methods that need them are dropped; only
// `/dev/kvm`-free tests are kept, so this module has no tests).

use std::mem::offset_of;
use std::sync::Arc;

use kvm_bindings::*;
use kvm_ioctls::{VcpuExit, VcpuFd, VmFd};

use super::EntryPoint;
use super::get_fdt_addr;
use super::kvm::OptionalCapabilities;
use super::regs::{KVM_REG_ARM_PTIMER_CNT, MPIDR_EL1, PSTATE_FAULT_BITS_64, arm64_core_reg_id};
use crate::vstate::bus::Bus;
use crate::vstate::memory::{Address, GuestMemoryMmap};
use crate::vstate::vcpu::{VcpuEmulation, VcpuError};
use crate::vstate::vm::KvmVm;

/// Errors thrown while setting aarch64 registers.
#[derive(Debug, thiserror::Error)]
pub enum VcpuArchError {
    /// Failed to get register {0}: {1}
    #[error("Failed to get register {0}: {1}")]
    GetOneReg(u64, kvm_ioctls::Error),
    /// Failed to set register {0:#x} to value {1}: {2}
    #[error("Failed to set register {0:#x} to value {1}: {2}")]
    SetOneReg(u64, String, kvm_ioctls::Error),
}

/// Errors associated with the wrappers over KVM ioctls.
#[derive(Debug, thiserror::Error)]
pub enum KvmVcpuError {
    /// Error configuring the vcpu registers: {0}
    #[error("Error configuring the vcpu registers: {0}")]
    ConfigureRegisters(VcpuArchError),
    /// Error creating vcpu: {0}
    #[error("Error creating vcpu: {0}")]
    CreateVcpu(kvm_ioctls::Error),
    /// Error getting the vcpu preferred target: {0}
    #[error("Error getting the vcpu preferred target: {0}")]
    GetPreferredTarget(kvm_ioctls::Error),
    /// Error initializing the vcpu: {0}
    #[error("Error initializing the vcpu: {0}")]
    Init(kvm_ioctls::Error),
}

/// Error type for [`KvmVcpu::configure`] (upstream aliases the two types).
pub type KvmVcpuConfigureError = KvmVcpuError;

/// A wrapper around creating and using a kvm aarch64 vcpu.
#[derive(Debug)]
pub struct KvmVcpu {
    /// Index of vcpu.
    pub index: u8,
    /// KVM vcpu fd.
    pub fd: VcpuFd,
    /// Vcpu peripherals, such as buses
    pub peripherals: Peripherals,
    /// The KVM vCPU initialization structure the vCPU was (or will be)
    /// initialized with: the default target plus PSCI 0.2, and
    /// `KVM_ARM_VCPU_POWER_OFF` for secondary vCPUs.
    kvi: kvm_vcpu_init,
}

/// Vcpu peripherals
#[derive(Default, Debug)]
pub struct Peripherals {
    /// mmio bus.
    pub mmio_bus: Option<Arc<Bus>>,
}

impl KvmVcpu {
    /// Constructs a new kvm vcpu with arch specific functionality.
    ///
    /// # Arguments
    ///
    /// * `index` - Represents the 0-based CPU index between [0, max vcpus).
    /// * `vm` - The vm to which this vcpu will get attached.
    pub fn new(index: u8, vm: &KvmVm) -> Result<Self, KvmVcpuError> {
        let kvm_vcpu = vm
            .fd()
            .create_vcpu(index.into())
            .map_err(KvmVcpuError::CreateVcpu)?;

        let mut kvi = Self::default_kvi(vm.fd())?;
        // Secondary vcpus must be powered off for boot process.
        if 0 < index {
            kvi.features[0] |= 1 << KVM_ARM_VCPU_POWER_OFF;
        }

        Ok(KvmVcpu {
            index,
            fd: kvm_vcpu,
            peripherals: Default::default(),
            kvi,
        })
    }

    /// Read the MPIDR - Multiprocessor Affinity Register.
    pub fn get_mpidr(&self) -> Result<u64, VcpuArchError> {
        // MPIDR register is 64 bit wide on aarch64
        let mut mpidr = [0_u8; 8];
        match self.fd.get_one_reg(MPIDR_EL1, &mut mpidr) {
            Err(err) => Err(VcpuArchError::GetOneReg(MPIDR_EL1, err)),
            Ok(_) => Ok(u64::from_le_bytes(mpidr)),
        }
    }

    /// Configures an aarch64 specific vcpu for booting Linux.
    ///
    /// # Arguments
    ///
    /// * `guest_mem` - The guest memory used by this microvm.
    /// * `kernel_entry_point` - Specifies the boot protocol and offset from `guest_mem` at which
    ///   the kernel starts.
    /// * `optional_capabilities` - Which optional capabilities are enabled that might influence
    ///   vcpu configuration.
    pub fn configure(
        &mut self,
        guest_mem: &GuestMemoryMmap,
        kernel_entry_point: EntryPoint,
        optional_capabilities: &OptionalCapabilities,
    ) -> Result<(), KvmVcpuError> {
        // Upstream applies the CPU template's register modifiers here; this
        // port has no templates, so only the boot registers are set.
        self.setup_boot_regs(
            kernel_entry_point.entry_addr.raw_value(),
            guest_mem,
            optional_capabilities,
        )
        .map_err(KvmVcpuError::ConfigureRegisters)?;

        Ok(())
    }

    /// Initializes an aarch64 specific vcpu for booting Linux (`KVM_ARM_VCPU_INIT`).
    ///
    /// Upstream takes the CPU template's `VcpuFeatures` and, after the
    /// initialization, finalizes the vCPU when SVE is enabled; this port has no
    /// templates, so the vCPU is initialized with the default features
    /// (PSCI 0.2, plus `KVM_ARM_VCPU_POWER_OFF` on secondary vCPUs) and no
    /// further finalization is needed. `KVM_ARM_VCPU_PMU_V3` must not be set:
    /// KVM fails `KVM_RUN` with `EINVAL` unless the PMU is initialized through
    /// `KVM_SET_DEVICE_ATTR`, which this port does not do, and no vCPU here
    /// ever requests it.
    pub fn init(&mut self) -> Result<(), KvmVcpuError> {
        self.fd.vcpu_init(&self.kvi).map_err(KvmVcpuError::Init)?;

        Ok(())
    }

    /// Creates default kvi struct based on vcpu index.
    pub fn default_kvi(vm_fd: &VmFd) -> Result<kvm_vcpu_init, KvmVcpuError> {
        let mut kvi = kvm_vcpu_init::default();
        // This reads back the kernel's preferred target type.
        vm_fd
            .get_preferred_target(&mut kvi)
            .map_err(KvmVcpuError::GetPreferredTarget)?;
        // We already checked that the capability is supported.
        kvi.features[0] |= 1 << KVM_ARM_VCPU_PSCI_0_2;

        Ok(kvi)
    }

    /// Configure relevant boot registers for a given vCPU.
    ///
    /// # Arguments
    ///
    /// * `boot_ip` - Starting instruction pointer.
    /// * `mem` - Reserved DRAM for current VM.
    /// * `optional_capabilities` - Which optional capabilities are enabled that might influence
    ///   vcpu configuration.
    pub fn setup_boot_regs(
        &self,
        boot_ip: u64,
        mem: &GuestMemoryMmap,
        optional_capabilities: &OptionalCapabilities,
    ) -> Result<(), VcpuArchError> {
        let kreg_off = offset_of!(kvm_regs, regs);

        // Get the register index of the PSTATE (Processor State) register.
        let pstate = offset_of!(user_pt_regs, pstate) + kreg_off;
        let id = arm64_core_reg_id!(KVM_REG_SIZE_U64, pstate);
        self.fd
            .set_one_reg(id, &PSTATE_FAULT_BITS_64.to_le_bytes())
            .map_err(|err| {
                VcpuArchError::SetOneReg(id, format!("{PSTATE_FAULT_BITS_64:#x}"), err)
            })?;

        // Other vCPUs are powered off initially awaiting PSCI wakeup.
        if self.index == 0 {
            // Setting the PC (Program Counter) to the current program address (kernel address).
            let pc = offset_of!(user_pt_regs, pc) + kreg_off;
            let id = arm64_core_reg_id!(KVM_REG_SIZE_U64, pc);
            self.fd
                .set_one_reg(id, &boot_ip.to_le_bytes())
                .map_err(|err| VcpuArchError::SetOneReg(id, format!("{boot_ip:#x}"), err))?;

            // Last mandatory thing to set -> the address pointing to the FDT (also called DTB).
            // "The device tree blob (dtb) must be placed on an 8-byte boundary and must
            // not exceed 2 megabytes in size." -> https://www.kernel.org/doc/Documentation/arm64/booting.txt.
            // We are choosing to place it the end of DRAM. See `get_fdt_addr`.
            let regs0 = offset_of!(user_pt_regs, regs) + kreg_off;
            let id = arm64_core_reg_id!(KVM_REG_SIZE_U64, regs0);
            let fdt_addr = get_fdt_addr(mem);
            self.fd
                .set_one_reg(id, &fdt_addr.to_le_bytes())
                .map_err(|err| VcpuArchError::SetOneReg(id, format!("{fdt_addr:#x}"), err))?;

            // Reset the physical counter for the guest. This way we avoid guest reading
            // host physical counter.
            // Resetting KVM_REG_ARM_PTIMER_CNT for single vcpu is enough because there is only
            // one timer struct with offsets per VM.
            // Because the access to KVM_REG_ARM_PTIMER_CNT is only present starting 6.4 kernel,
            // we only do the reset if KVM_CAP_COUNTER_OFFSET is present as it was added
            // in the same patch series as the ability to set the KVM_REG_ARM_PTIMER_CNT register.
            // Path series which introduced the needed changes:
            // https://lore.kernel.org/all/20230330174800.2677007-1-maz@kernel.org/
            // Note: the value observed by the guest will still be above 0, because there is a delta
            // time between this resetting and first call to KVM_RUN.
            if optional_capabilities.counter_offset {
                self.fd
                    .set_one_reg(KVM_REG_ARM_PTIMER_CNT, &[0; 8])
                    .map_err(|err| {
                        VcpuArchError::SetOneReg(id, format!("{KVM_REG_ARM_PTIMER_CNT:#x}"), err)
                    })?;
            }
        }
        Ok(())
    }
}

impl Peripherals {
    /// Runs the vCPU in KVM context and handles the kvm exit reason.
    ///
    /// The arch-independent run loop in `vstate::vcpu` handles the exits that
    /// occur on both architectures (MMIO, system events, the error exits); on
    /// KVM/aarch64 everything else, WFI and WFE included, is handled inside the
    /// kernel and does not reach userspace, so any reason arriving here is
    /// unexpected.
    pub fn run_arch_emulation(&self, exit: VcpuExit) -> Result<VcpuEmulation, VcpuError> {
        Err(VcpuError::UnhandledKvmExit(format!("{exit:?}")))
    }
}
