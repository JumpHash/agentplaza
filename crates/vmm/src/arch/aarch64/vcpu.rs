// Copyright 2023 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/arch/aarch64/vcpu.rs (trimmed: the
// CPU-template machinery (`VcpuConfig`, `CpuConfiguration`,
// `RegisterValueFilter`, `VcpuFeatures`), SVE finalization, the pvtime
// (steal-time) support, the metrics/logger crates and the `Peripherals` methods
// that need them are dropped. The snapshot state keeps the MP state, every
// register `KVM_GET_REG_LIST` lists (as plain byte vectors, not Firecracker's
// packed `Aarch64RegisterVec`), the MPIDR and the init features; with no
// templates or SVE, nothing else needs saving. Only `/dev/kvm`-free tests are
// kept, so this module has no tests that need a VM.

use std::mem::offset_of;
use std::sync::Arc;

use kvm_bindings::*;
use kvm_ioctls::{VcpuExit, VcpuFd, VmFd};

use super::EntryPoint;
use super::get_fdt_addr;
use super::kvm::OptionalCapabilities;
use super::regs::{
    KVM_REG_ARM_PTIMER_CNT, MPIDR_EL1, PSTATE_FAULT_BITS_64, arm64_core_reg_id, reg_size,
};
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
    /// Failed to retrieve list of registers: {0}
    #[error("Failed to retrieve list of registers: {0}")]
    GetRegList(kvm_ioctls::Error),
    /// Failed to get multiprocessor state: {0}
    #[error("Failed to get multiprocessor state: {0}")]
    GetMp(kvm_ioctls::Error),
    /// Failed to set multiprocessor state: {0}
    #[error("Failed to set multiprocessor state: {0}")]
    SetMp(kvm_ioctls::Error),
    /// Failed to allocate the register list: {0}
    #[error("Failed to allocate the register list: {0}")]
    Fam(vmm_sys_util::fam::Error),
    /// Register {0:#x} holds {1} bytes, but its ID encodes {2}
    #[error("Register {0:#x} holds {1} bytes, but its ID encodes {2}")]
    InvalidRegisterSize(u64, usize, usize),
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
    /// Failed to save the state of the vcpu: {0}
    #[error("Failed to save the state of the vcpu: {0}")]
    SaveState(VcpuArchError),
    /// Failed to restore the state of the vcpu: {0}
    #[error("Failed to restore the state of the vcpu: {0}")]
    RestoreState(VcpuArchError),
    /// Found unsupported KVM_ARM_VCPU_PMU_V3 bit set in vcpu features.
    #[error("Found unsupported KVM_ARM_VCPU_PMU_V3 bit set in vcpu features.")]
    UnsupportedPmuV3,
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

/// One register of a vCPU snapshot: its KVM ID and the bytes `KVM_GET_ONE_REG` returned for it.
/// The length is the size the ID encodes ([`reg_size`]); restore checks it, because KVM reads
/// exactly that many bytes from the buffer it is given.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VcpuRegister {
    /// The KVM register ID.
    pub id: u64,
    /// The register's value, little-endian as KVM stores it.
    pub data: Vec<u8>,
}

/// The state of one aarch64 vCPU in a snapshot (Firecracker's `VcpuState`): the MP state, every
/// register KVM lists for the vCPU (`KVM_GET_REG_LIST`), its MPIDR and the KVM vCPU init features.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VcpuState {
    /// The multiprocessing state (`KVM_GET_MP_STATE`).
    pub mp_state: kvm_mp_state,
    /// Every register `KVM_GET_REG_LIST` lists for the vCPU.
    pub regs: Vec<VcpuRegister>,
    /// The vCPU's MPIDR. The VM state keys the GIC's per-vCPU registers by it.
    pub mpidr: u64,
    /// The `KVM_ARM_VCPU_INIT` features the vCPU is restored with.
    pub kvi: kvm_vcpu_init,
}

impl KvmVcpu {
    /// Saves this vCPU's state. The vCPU must be paused (out of `KVM_RUN`).
    pub fn save_state(&self) -> Result<VcpuState, KvmVcpuError> {
        let mp_state = self
            .fd
            .get_mp_state()
            .map_err(|err| KvmVcpuError::SaveState(VcpuArchError::GetMp(err)))?;
        let regs = self.get_all_registers().map_err(KvmVcpuError::SaveState)?;
        let mpidr = self.get_mpidr().map_err(KvmVcpuError::SaveState)?;

        // The power-off bit only parks a secondary vCPU during the boot. A snapshot is taken
        // after that, and the kernel has already turned the vCPU on, so restoring it powered
        // off would stop a running vCPU.
        let mut kvi = self.kvi;
        kvi.features[0] &= !(1 << KVM_ARM_VCPU_POWER_OFF);

        Ok(VcpuState {
            mp_state,
            regs,
            mpidr,
            kvi,
        })
    }

    /// Restores a state [`KvmVcpu::save_state`] produced.
    ///
    /// This performs `KVM_ARM_VCPU_INIT` itself, with the saved features, so the vCPU must not
    /// have been initialized yet, and it must run this before it first runs. The registers and
    /// the MP state are written after the initialization, in the order the snapshot lists them.
    pub fn restore_state(&mut self, state: &VcpuState) -> Result<(), KvmVcpuError> {
        // Everything that can refuse the snapshot is checked before the vCPU is initialized, so
        // a bad snapshot leaves the vCPU untouched.
        for reg in &state.regs {
            check_register_size(reg).map_err(KvmVcpuError::RestoreState)?;
        }
        if (state.kvi.features[0] & (1 << KVM_ARM_VCPU_PMU_V3)) != 0 {
            return Err(KvmVcpuError::UnsupportedPmuV3);
        }

        self.kvi = state.kvi;
        self.init()?;

        for reg in &state.regs {
            self.set_register(reg).map_err(KvmVcpuError::RestoreState)?;
        }
        self.fd
            .set_mp_state(state.mp_state)
            .map_err(|err| KvmVcpuError::RestoreState(VcpuArchError::SetMp(err)))?;

        Ok(())
    }

    /// Reads every register `KVM_GET_REG_LIST` lists for this vCPU.
    fn get_all_registers(&self) -> Result<Vec<VcpuRegister>, VcpuArchError> {
        // A register is at most 2048 bits wide (the SVE vector slices), so 256 bytes hold any.
        let mut value = [0_u8; 256];
        let mut regs = Vec::new();
        for id in self.get_all_registers_ids()? {
            let size = self
                .fd
                .get_one_reg(id, &mut value)
                .map_err(|err| VcpuArchError::GetOneReg(id, err))?;
            regs.push(VcpuRegister {
                id,
                data: value[..size].to_vec(),
            });
        }
        Ok(regs)
    }

    /// Returns the IDs of all registers the vCPU has, core and system registers included.
    fn get_all_registers_ids(&self) -> Result<Vec<u64>, VcpuArchError> {
        // `KVM_GET_REG_LIST` fails with `E2BIG` when the list does not fit, and then writes the
        // number of entries it needs into `n`, so one retry at that size is enough. For ARMv8
        // there are fewer than 500 registers, so the first attempt almost always fits.
        let mut reg_list = RegList::new(500).map_err(VcpuArchError::Fam)?;
        match self.fd.get_reg_list(&mut reg_list) {
            Ok(()) => Ok(reg_list.as_slice().to_vec()),
            Err(err) if err.errno() == libc::E2BIG => {
                let size = usize::try_from(reg_list.as_fam_struct_ref().n)
                    .map_err(|_| VcpuArchError::GetRegList(err))?;
                reg_list = RegList::new(size).map_err(VcpuArchError::Fam)?;
                self.fd
                    .get_reg_list(&mut reg_list)
                    .map_err(VcpuArchError::GetRegList)?;
                Ok(reg_list.as_slice().to_vec())
            }
            Err(err) => Err(VcpuArchError::GetRegList(err)),
        }
    }

    /// Writes one register back with `KVM_SET_ONE_REG`.
    fn set_register(&self, reg: &VcpuRegister) -> Result<(), VcpuArchError> {
        self.fd
            .set_one_reg(reg.id, &reg.data)
            .map_err(|err| VcpuArchError::SetOneReg(reg.id, reg_value_str(&reg.data), err))?;
        Ok(())
    }
}

/// Checks that a register's data is exactly as wide as its ID encodes.
fn check_register_size(reg: &VcpuRegister) -> Result<(), VcpuArchError> {
    let expected = reg_size(reg.id);
    if reg.data.len() != expected {
        return Err(VcpuArchError::InvalidRegisterSize(
            reg.id,
            reg.data.len(),
            expected,
        ));
    }
    Ok(())
}

/// Formats a register value as a hex number, reading its little-endian bytes from the top.
fn reg_value_str(data: &[u8]) -> String {
    let hex: String = data
        .iter()
        .rev()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("0x{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A register whose data is not as wide as its ID encodes is refused, since KVM would read
    /// past the buffer it is given.
    #[test]
    fn register_data_must_match_the_width_its_id_encodes() {
        let full = VcpuRegister {
            id: MPIDR_EL1,
            data: vec![0; 8],
        };
        assert!(check_register_size(&full).is_ok());

        let short = VcpuRegister {
            id: MPIDR_EL1,
            data: vec![0; 4],
        };
        assert!(matches!(
            check_register_size(&short),
            Err(VcpuArchError::InvalidRegisterSize(MPIDR_EL1, 4, 8))
        ));
    }

    /// The snapshot survives serde, including the KVM structures it carries.
    #[test]
    fn vcpu_state_round_trips_through_serde() {
        let mut kvi = kvm_vcpu_init::default();
        kvi.features[0] = 1 << KVM_ARM_VCPU_PSCI_0_2;
        let state = VcpuState {
            mp_state: kvm_mp_state::default(),
            regs: vec![VcpuRegister {
                id: MPIDR_EL1,
                data: 0x8000_0000_u64.to_le_bytes().to_vec(),
            }],
            mpidr: 0x8000_0000,
            kvi,
        };

        let json = serde_json::to_string(&state).unwrap();
        let restored: VcpuState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, state);
    }

    /// The hex form reads the little-endian bytes from the most significant end.
    #[test]
    fn register_value_string_is_big_endian_hex() {
        assert_eq!(reg_value_str(&[0x78, 0x56, 0x34, 0x12]), "0x12345678");
    }
}
