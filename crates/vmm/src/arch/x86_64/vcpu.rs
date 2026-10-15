// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/arch/x86_64/vcpu.rs (trimmed: the
// CPU-template/CPU-config machinery is dropped — `configure_cpuid` installs
// KVM's supported CPUID with the per-CPU topology normalization inlined from
// `cpu_config/x86_64/cpuid/normalize.rs` instead of applying a template; the
// TSC-offset and MSR-dump helpers are dropped; the snapshot save/restore is
// ported with the serializable MSR filter from `arch/x86_64/msr.rs`).

use std::sync::Arc;

use kvm_bindings::{
    CpuId, KVM_CPUID_FLAG_SIGNIFCANT_INDEX, KVM_MAX_CPUID_ENTRIES, KVM_MAX_MSR_ENTRIES, Msrs,
    Xsave, kvm_debugregs, kvm_lapic_state, kvm_mp_state, kvm_regs, kvm_sregs, kvm_vcpu_events,
    kvm_xcrs, kvm_xsave, kvm_xsave2,
};
use kvm_ioctls::{Cap, VcpuExit, VcpuFd};

use crate::arch::x86_64::EntryPoint;
use crate::arch::x86_64::interrupts;
use crate::arch::x86_64::kvm::Kvm;
use crate::arch::x86_64::msr::{MsrError, create_boot_msr_entries, set_msrs};
use crate::arch::x86_64::regs::{SetupFpuError, SetupRegistersError, SetupSpecialRegistersError};
use crate::vstate::bus::Bus;
use crate::vstate::memory::GuestMemoryMmap;
use crate::vstate::vcpu::{VcpuEmulation, VcpuError};
use crate::vstate::vm::KvmVm;

// Maximum number of vCPUs supported per VM (upstream: the machine-config limit
// from `vmm_config/machine_config.rs`; used to keep the CPUID.(EAX=0BH,ECX=1)
// topology stable regardless of the configured vCPU count).
pub const MAX_SUPPORTED_VCPUS: u8 = 32;

// Tolerance for TSC frequency expected variation.
// The value of 250 parts per million is based on
// the QEMU approach, more details here:
// https://bugzilla.redhat.com/show_bug.cgi?id=1839095
const TSC_KHZ_TOL_NUMERATOR: i64 = 250;
const TSC_KHZ_TOL_DENOMINATOR: i64 = 1_000_000;

/// The MSR indices the serializable list names. The generated `msr_index` module carries only the
/// MSRs the rest of the crate uses, so these are the Linux `msr-index.h` values (as in Firecracker's
/// generated module).
mod msr_consts {
    pub const MSR_IA32_P5_MC_ADDR: u32 = 0x0;
    pub const MSR_IA32_P5_MC_TYPE: u32 = 0x1;
    pub const MSR_IA32_TSC: u32 = 0x10;
    pub const MSR_IA32_PLATFORM_ID: u32 = 0x17;
    pub const MSR_IA32_APICBASE: u32 = 0x1b;
    pub const MSR_IA32_EBL_CR_POWERON: u32 = 0x2a;
    pub const MSR_EBC_FREQUENCY_ID: u32 = 0x2c;
    pub const MSR_SMI_COUNT: u32 = 0x34;
    pub const MSR_IA32_FEAT_CTL: u32 = 0x3a;
    pub const MSR_IA32_TSC_ADJUST: u32 = 0x3b;
    pub const MSR_IA32_SPEC_CTRL: u32 = 0x48;
    pub const MSR_IA32_PRED_CMD: u32 = 0x49;
    pub const MSR_IA32_UCODE_WRITE: u32 = 0x79;
    pub const MSR_IA32_UCODE_REV: u32 = 0x8b;
    pub const MSR_IA32_SMBASE: u32 = 0x9e;
    pub const MSR_FSB_FREQ: u32 = 0xcd;
    pub const MSR_PLATFORM_INFO: u32 = 0xce;
    pub const MSR_PKG_CST_CONFIG_CONTROL: u32 = 0xe2;
    pub const MSR_IA32_MPERF: u32 = 0xe7;
    pub const MSR_IA32_APERF: u32 = 0xe8;
    pub const MSR_MTRR_CAP: u32 = 0xfe;
    pub const MSR_IA32_BBL_CR_CTL3: u32 = 0x11e;
    pub const MSR_IA32_TSX_CTRL: u32 = 0x122;
    pub const MSR_IA32_SYSENTER_CS: u32 = 0x174;
    pub const MSR_IA32_SYSENTER_ESP: u32 = 0x175;
    pub const MSR_IA32_SYSENTER_EIP: u32 = 0x176;
    pub const MSR_IA32_MCG_CAP: u32 = 0x179;
    pub const MSR_IA32_MCG_STATUS: u32 = 0x17a;
    pub const MSR_IA32_MCG_CTL: u32 = 0x17b;
    pub const MSR_IA32_PERF_STATUS: u32 = 0x198;
    pub const MSR_IA32_MISC_ENABLE: u32 = 0x1a0;
    pub const MSR_MISC_FEATURE_CONTROL: u32 = 0x1a4;
    pub const MSR_MISC_PWR_MGMT: u32 = 0x1aa;
    pub const MSR_TURBO_RATIO_LIMIT: u32 = 0x1ad;
    pub const MSR_TURBO_RATIO_LIMIT1: u32 = 0x1ae;
    pub const MSR_IA32_DEBUGCTLMSR: u32 = 0x1d9;
    pub const MSR_IA32_LASTBRANCHFROMIP: u32 = 0x1db;
    pub const MSR_IA32_LASTBRANCHTOIP: u32 = 0x1dc;
    pub const MSR_IA32_LASTINTFROMIP: u32 = 0x1dd;
    pub const MSR_IA32_LASTINTTOIP: u32 = 0x1de;
    pub const MSR_IA32_POWER_CTL: u32 = 0x1fc;
    pub const MSR_CORE_C3_RESIDENCY: u32 = 0x3fc;
    pub const MSR_IA32_MC0_CTL: u32 = 0x400;
    pub const MSR_RAPL_POWER_UNIT: u32 = 0x606;
    pub const MSR_PKGC3_IRTL: u32 = 0x60a;
    pub const MSR_PKG_POWER_LIMIT: u32 = 0x610;
    pub const MSR_PKG_ENERGY_STATUS: u32 = 0x611;
    pub const MSR_PKG_PERF_STATUS: u32 = 0x613;
    pub const MSR_PKG_POWER_INFO: u32 = 0x614;
    pub const MSR_DRAM_POWER_LIMIT: u32 = 0x618;
    pub const MSR_DRAM_ENERGY_STATUS: u32 = 0x619;
    pub const MSR_DRAM_PERF_STATUS: u32 = 0x61b;
    pub const MSR_DRAM_POWER_INFO: u32 = 0x61c;
    pub const MSR_CONFIG_TDP_NOMINAL: u32 = 0x648;
    pub const MSR_CONFIG_TDP_LEVEL_1: u32 = 0x649;
    pub const MSR_CONFIG_TDP_LEVEL_2: u32 = 0x64a;
    pub const MSR_CONFIG_TDP_CONTROL: u32 = 0x64b;
    pub const MSR_TURBO_ACTIVATION_RATIO: u32 = 0x64c;
    pub const MSR_IA32_TSC_DEADLINE: u32 = 0x6e0;
    pub const MSR_EFER: u32 = 0xc000_0080;
    pub const MSR_STAR: u32 = 0xc000_0081;
    pub const MSR_LSTAR: u32 = 0xc000_0082;
    pub const MSR_CSTAR: u32 = 0xc000_0083;
    pub const MSR_SYSCALL_MASK: u32 = 0xc000_0084;
    pub const MSR_FS_BASE: u32 = 0xc000_0100;
    pub const MSR_GS_BASE: u32 = 0xc000_0101;
    pub const MSR_KERNEL_GS_BASE: u32 = 0xc000_0102;
    pub const MSR_TSC_AUX: u32 = 0xc000_0103;
    pub const MSR_MISC_FEATURES_ENABLES: u32 = 0x140;
    pub const MSR_K7_HWCR: u32 = 0xc001_0015;
    pub const MSR_IA32_XSS: u32 = 0xda0;
}
use msr_consts::*;

/// A set of MSRs that should be restored separately after all other MSRs have already been restored
const DEFERRED_MSRS: [u32; 1] = [
    // MSR_IA32_TSC_DEADLINE must be restored after MSR_IA32_TSC, otherwise we risk "losing" timer
    // interrupts across the snapshot restore boundary (due to KVM querying MSR_IA32_TSC upon
    // writes to the TSC_DEADLINE MSR to determine whether it needs to prime a timer - if
    // MSR_IA32_TSC is not initialized correctly, it can wrongly assume no timer needs to be
    // primed, or the timer can be initialized with a wrong expiry).
    MSR_IA32_TSC_DEADLINE,
];

/// Errors associated with the wrappers over KVM ioctls.
#[derive(Debug, thiserror::Error)]
pub enum KvmVcpuError {
    /// Cannot open the VCPU file descriptor: {0}
    #[error("Cannot open the VCPU file descriptor: {0}")]
    VcpuFd(kvm_ioctls::Error),
    /// Failed to get the KVM MSR index list: {0}
    #[error("Failed to get the KVM MSR index list: {0}")]
    GetMsrIndexList(kvm_ioctls::Error),
    /// Failed FamStructWrapper operation: {0}
    #[error("Failed FamStructWrapper operation: {0}")]
    Fam(vmm_sys_util::fam::Error),
    /// Failed to get KVM vcpu cpuid: {0}
    #[error("Failed to get KVM vcpu cpuid: {0}")]
    VcpuGetCpuid(kvm_ioctls::Error),
    /// Failed to get KVM vcpu debug regs: {0}
    #[error("Failed to get KVM vcpu debug regs: {0}")]
    VcpuGetDebugRegs(kvm_ioctls::Error),
    /// Failed to get KVM vcpu lapic: {0}
    #[error("Failed to get KVM vcpu lapic: {0}")]
    VcpuGetLapic(kvm_ioctls::Error),
    /// Failed to get KVM vcpu mp state: {0}
    #[error("Failed to get KVM vcpu mp state: {0}")]
    VcpuGetMpState(kvm_ioctls::Error),
    /// Failed to get KVM vcpu msr: {0:#x}
    #[error("Failed to get KVM vcpu msr: {0:#x}")]
    VcpuGetMsr(u32),
    /// Failed to get KVM vcpu msrs: {0}
    #[error("Failed to get KVM vcpu msrs: {0}")]
    VcpuGetMsrs(kvm_ioctls::Error),
    /// Failed to get KVM vcpu regs: {0}
    #[error("Failed to get KVM vcpu regs: {0}")]
    VcpuGetRegs(kvm_ioctls::Error),
    /// Failed to get KVM vcpu sregs: {0}
    #[error("Failed to get KVM vcpu sregs: {0}")]
    VcpuGetSregs(kvm_ioctls::Error),
    /// Failed to get KVM vcpu event: {0}
    #[error("Failed to get KVM vcpu event: {0}")]
    VcpuGetVcpuEvents(kvm_ioctls::Error),
    /// Failed to get KVM vcpu xcrs: {0}
    #[error("Failed to get KVM vcpu xcrs: {0}")]
    VcpuGetXcrs(kvm_ioctls::Error),
    /// Failed to get KVM vcpu xsave via KVM_GET_XSAVE: {0}
    #[error("Failed to get KVM vcpu xsave via KVM_GET_XSAVE: {0}")]
    VcpuGetXsave(kvm_ioctls::Error),
    /// Failed to get KVM vcpu xsave via KVM_GET_XSAVE2: {0}
    #[error("Failed to get KVM vcpu xsave via KVM_GET_XSAVE2: {0}")]
    VcpuGetXsave2(kvm_ioctls::Error),
    /// Failed to get KVM TSC frequency: {0}
    #[error("Failed to get KVM TSC frequency: {0}")]
    VcpuGetTsc(kvm_ioctls::Error),
    /// Failed to set KVM vcpu cpuid: {0}
    #[error("Failed to set KVM vcpu cpuid: {0}")]
    VcpuSetCpuid(kvm_ioctls::Error),
    /// Failed to set KVM vcpu debug regs: {0}
    #[error("Failed to set KVM vcpu debug regs: {0}")]
    VcpuSetDebugRegs(kvm_ioctls::Error),
    /// Failed to set KVM vcpu lapic: {0}
    #[error("Failed to set KVM vcpu lapic: {0}")]
    VcpuSetLapic(kvm_ioctls::Error),
    /// Failed to set KVM vcpu mp state: {0}
    #[error("Failed to set KVM vcpu mp state: {0}")]
    VcpuSetMpState(kvm_ioctls::Error),
    /// Failed to set KVM vcpu msrs: {0}
    #[error("Failed to set KVM vcpu msrs: {0}")]
    VcpuSetMsrs(kvm_ioctls::Error),
    /// Failed to set all KVM MSRs for this vCPU. Only a partial write was done.
    #[error("Failed to set all KVM MSRs for this vCPU. Only a partial write was done.")]
    VcpuSetMsrsIncomplete,
    /// Failed to set KVM vcpu regs: {0}
    #[error("Failed to set KVM vcpu regs: {0}")]
    VcpuSetRegs(kvm_ioctls::Error),
    /// Failed to set KVM vcpu sregs: {0}
    #[error("Failed to set KVM vcpu sregs: {0}")]
    VcpuSetSregs(kvm_ioctls::Error),
    /// Failed to set KVM TSC frequency: {0}
    #[error("Failed to set KVM TSC frequency: {0}")]
    VcpuSetTsc(kvm_ioctls::Error),
    /// Failed to set KVM vcpu event: {0}
    #[error("Failed to set KVM vcpu event: {0}")]
    VcpuSetVcpuEvents(kvm_ioctls::Error),
    /// Failed to set KVM vcpu xcrs: {0}
    #[error("Failed to set KVM vcpu xcrs: {0}")]
    VcpuSetXcrs(kvm_ioctls::Error),
    /// Failed to set KVM vcpu xsave: {0}
    #[error("Failed to set KVM vcpu xsave: {0}")]
    VcpuSetXsave(kvm_ioctls::Error),
}

/// Errors associated with configuring an x86_64 vCPU.
#[derive(Debug, thiserror::Error)]
pub enum KvmVcpuConfigureError {
    /// Failed to set CPUID: {0}
    #[error("Failed to set CPUID: {0}")]
    SetCpuid(#[from] kvm_ioctls::Error),
    /// Failed to set MSRs: {0}
    #[error("Failed to set MSRs: {0}")]
    SetMsrs(#[from] MsrError),
    /// Failed to setup registers: {0}
    #[error("Failed to setup registers: {0}")]
    SetupRegisters(#[from] SetupRegistersError),
    /// Failed to setup FPU: {0}
    #[error("Failed to setup FPU: {0}")]
    SetupFpu(#[from] SetupFpuError),
    /// Failed to setup special registers: {0}
    #[error("Failed to setup special registers: {0}")]
    SetupSpecialRegisters(#[from] SetupSpecialRegistersError),
    /// Failed to configure LAPICs: {0}
    #[error("Failed to configure LAPICs: {0}")]
    SetLint(#[from] interrupts::InterruptError),
}

/// A wrapper around creating and using a kvm x86_64 vcpu.
#[derive(Debug)]
pub struct KvmVcpu {
    /// Index of vcpu.
    pub index: u8,
    /// KVM vcpu fd.
    pub fd: VcpuFd,
    /// Vcpu peripherals, such as buses
    pub peripherals: Peripherals,
    /// The MSRs a snapshot saves, in the order KVM returned them from `KVM_GET_MSR_INDEX_LIST`,
    /// filtered to the serializable ranges (see [`msr_should_serialize`]).
    msrs_to_save: Vec<u32>,
    /// Size in bytes of the `kvm_xsave2` area, or `None` without `KVM_CAP_XSAVE2`. With it, the
    /// XSAVE area includes dynamic features (AMX tile data) that do not fit `kvm_xsave`'s 4 KiB.
    xsave2_size: Option<usize>,
}

/// Vcpu peripherals
#[derive(Default, Debug)]
pub struct Peripherals {
    /// Pio bus.
    pub pio_bus: Option<Arc<Bus>>,
    /// Mmio bus.
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
            .create_vcpu(u64::from(index))
            .map_err(KvmVcpuError::VcpuFd)?;
        // The MSR list and the XSAVE size come from the VM's KVM handle, so no second
        // `/dev/kvm` is opened here (a jailed VM cannot open one).
        let kvm = vm.kvm();

        Ok(KvmVcpu {
            index,
            fd: kvm_vcpu,
            peripherals: Default::default(),
            msrs_to_save: get_msrs_to_save(&kvm.fd)?,
            xsave2_size: xsave2_size(&kvm.fd),
        })
    }

    /// Normalizes and configures CPUID for this vCPU.
    ///
    /// Upstream applies a CPU template and then normalizes the topology
    /// per-vCPU through `cpu_config::x86_64::cpuid::normalize`; without
    /// templates, KVM's supported CPUID is installed directly with the
    /// per-CPU topology bits (initial APIC ID, HTT and the extended
    /// topology leaf 0xB) normalized inline.
    ///
    /// # Arguments
    ///
    /// * `kvm` - The shared KVM handle holding the supported CPUID.
    /// * `vcpu_count` - The total number of logical vCPUs.
    ///
    /// # Errors
    ///
    /// Returns an error if CPUID cannot be installed on the vCPU.
    pub fn configure_cpuid(
        &self,
        kvm: &Kvm,
        vcpu_count: u8,
        smt: bool,
    ) -> Result<(), KvmVcpuConfigureError> {
        let mut kvm_cpuid = kvm.supported_cpuid.clone();
        normalize_cpuid_topology(
            kvm_cpuid.as_mut_slice(),
            self.index,
            vcpu_count,
            u8::from(vcpu_count > 1 && smt),
        );
        self.fd
            .set_cpuid2(&kvm_cpuid)
            .map_err(KvmVcpuConfigureError::SetCpuid)?;
        Ok(())
    }

    /// Configures the Linux boot MSRs for this vCPU.
    ///
    /// Upstream additionally installs template-provided MSRs and extends the
    /// snapshot MSR list from CPUID; both are template/snapshot machinery and
    /// are trimmed here.
    ///
    /// # Errors
    ///
    /// Returns an error if the MSRs cannot be installed on the vCPU.
    pub fn configure_msrs_for_boot(&mut self) -> Result<(), KvmVcpuConfigureError> {
        let msrs = create_boot_msr_entries();
        set_msrs(&self.fd, &msrs)?;
        Ok(())
    }

    /// Configures the remaining Linux boot state after CPUID and MSRs for this vCPU.
    ///
    /// # Arguments
    ///
    /// * `guest_mem` - The guest memory used by the microVM.
    /// * `kernel_entry_point` - The boot protocol and guest address of the kernel entry point.
    ///
    /// # Errors
    ///
    /// Returns an error if the register, FPU, special register, or LINT setup fails.
    pub fn configure_boot_state(
        &self,
        guest_mem: &GuestMemoryMmap,
        kernel_entry_point: EntryPoint,
    ) -> Result<(), KvmVcpuConfigureError> {
        crate::arch::x86_64::regs::setup_regs(&self.fd, kernel_entry_point)?;
        crate::arch::x86_64::regs::setup_fpu(&self.fd)?;
        crate::arch::x86_64::regs::setup_sregs(guest_mem, &self.fd, kernel_entry_point.protocol)?;
        crate::arch::x86_64::interrupts::set_lint(&self.fd)?;
        Ok(())
    }

    /// Sets a Port Mapped IO bus for this vcpu.
    pub fn set_pio_bus(&mut self, pio_bus: Arc<Bus>) {
        self.peripherals.pio_bus = Some(pio_bus);
    }
}

/// Sets a given bit to a true or false (1 or 0).
fn set_bit(x: &mut u32, bit: u8, y: bool) {
    debug_assert!(bit < 32);
    *x = (*x & !(1 << bit)) | (u32::from(u8::from(y)) << bit);
}

/// Sets a given range to a given value.
fn set_range(x: &mut u32, range: std::ops::RangeInclusive<u8>, y: u32) {
    let start = *range.start();
    let end = *range.end();
    debug_assert!(end >= start);
    debug_assert!(end < 32);
    let num_bits = end - start + 1;
    // The callers of this trimmed helper pass values that fit (upstream
    // returns a `CheckedAssignError` when they do not).
    debug_assert!(num_bits == 32 || y < (1u32 << num_bits));
    let mask = ((1u32 << num_bits) - 1) << start;
    *x = (*x & !mask) | (y << start);
}

/// Normalizes the per-CPU topology bits of KVM's supported CPUID for the
/// vCPU with index `cpu_index` (inlined, trimmed version of upstream's
/// `cpu_config::x86_64::cpuid::normalize`: `update_feature_info_entry` and
/// `update_extended_topology_entry`).
fn normalize_cpuid_topology(
    cpuid: &mut [kvm_bindings::kvm_cpuid_entry2],
    cpu_index: u8,
    cpu_count: u8,
    cpu_bits: u8,
) {
    // Logical processors per core: 2 when SMT is enabled, 1 otherwise.
    let cpus_per_core = u8::from(cpu_bits > 0) + 1;

    for entry in cpuid.iter_mut() {
        if entry.index != 0 && entry.function != 0xB {
            continue;
        }
        match (entry.function, entry.index) {
            (0x1, 0) => {
                // CPUID.01H:EBX[23:16] — maximum number of addressable IDs
                // for logical processors in this physical package (the
                // nearest power-of-2 integer not smaller than the vCPU count).
                set_range(
                    &mut entry.ebx,
                    16..=23,
                    u32::from(cpu_count.max(1).next_power_of_two()),
                );
                // CPUID.01H:EBX[31:24] — initial APIC ID.
                set_range(&mut entry.ebx, 24..=31, u32::from(cpu_index));
                // CPUID.01H:ECX[24] (TSC-Deadline) and ECX[31] (Hypervisor).
                set_bit(&mut entry.ecx, 24, true);
                set_bit(&mut entry.ecx, 31, true);
                // CPUID.01H:EDX[28] (HTT): valid only with more than one vCPU.
                set_bit(&mut entry.edx, 28, cpu_count > 1);
            }
            (0xB, index) => {
                // CPUID.(EAX=0BH,ECX=N).EDX[31:0] — x2APIC ID of the current
                // logical processor.
                entry.eax = 0;
                entry.ebx = 0;
                entry.ecx = 0;
                entry.edx = u32::from(cpu_index);
                entry.flags |= KVM_CPUID_FLAG_SIGNIFCANT_INDEX;
                match index {
                    // Logical processor domain
                    0 => {
                        // CPUID.(EAX=0BH,ECX=0).EAX[4:0] — the number of bits
                        // the x2APIC ID shifts right to address the next
                        // higher-scoped domain (at most 1: max 2 logical
                        // processors per core, representable by one bit).
                        set_range(&mut entry.eax, 0..=4, u32::from(cpu_bits));
                        // CPUID.(EAX=0BH,ECX=0).EBX[15:0] — logical processors
                        // per core: 1, or 2 with SMT.
                        set_range(&mut entry.ebx, 0..=15, u32::from(cpus_per_core));
                        // Domain type identification value for logical processor.
                        set_range(&mut entry.ecx, 8..=15, 1);
                    }
                    // Core domain
                    1 => {
                        // The CPUID.(EAX=0BH,ECX=1).EAX[4:0] value must be an
                        // integer N such that 2^N is greater than or equal to
                        // the maximum number of vCPUs.
                        set_range(
                            &mut entry.eax,
                            0..=4,
                            u32::from(MAX_SUPPORTED_VCPUS.next_power_of_two().ilog2()),
                        );
                        // All logical processors fit in the next
                        // higher-scoped domain (the socket).
                        set_range(&mut entry.ebx, 0..=15, u32::from(cpu_count));
                        // The input ECX value and the domain type
                        // identification value for core.
                        set_range(&mut entry.ecx, 0..=7, index);
                        set_range(&mut entry.ecx, 8..=15, 2);
                    }
                    _ => {
                        // KVM no longer returns any subleaves greater than 1
                        // (kernel v6.2, backported to v5.10). Normalization
                        // only reads whatever KVM provided.
                        entry.ecx = index;
                    }
                }
            }
            _ => (),
        }
    }
}

impl Peripherals {
    /// Runs the vCPU in KVM context and handles the kvm exit reason.
    ///
    /// Returns error or enum specifying whether emulation was handled or interrupted.
    pub fn run_arch_emulation(&self, exit: VcpuExit) -> Result<VcpuEmulation, VcpuError> {
        match exit {
            VcpuExit::IoIn(addr, data) => {
                data.fill(0);
                if let Some(pio_bus) = &self.pio_bus {
                    if let Err(err) = pio_bus.read(u64::from(addr), data) {
                        eprintln!(
                            "warning: vcpu: IO read @ {addr:#x}:{:#x} failed: {err}",
                            data.len()
                        );
                    }
                }
                Ok(VcpuEmulation::Handled)
            }
            VcpuExit::IoOut(addr, data) => {
                if let Some(pio_bus) = &self.pio_bus {
                    if let Err(err) = pio_bus.write(u64::from(addr), data) {
                        eprintln!(
                            "warning: vcpu: IO write @ {addr:#x}:{:#x} failed: {err}",
                            data.len()
                        );
                    }
                }
                Ok(VcpuEmulation::Handled)
            }
            unexpected_exit => Err(VcpuError::UnhandledKvmExit(format!("{unexpected_exit:?}"))),
        }
    }
}

/// The state of one x86_64 vCPU in a snapshot (Firecracker's `VcpuState`): CPUID, the saved MSRs,
/// general and special registers, debug registers, LAPIC, MP state, XCRs, XSAVE area, vCPU events
/// and the TSC frequency.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct VcpuState {
    /// CPUID installed on the vCPU.
    #[serde(with = "cpuid_serde")]
    pub cpuid: CpuId,
    /// The saved MSRs in chunks of at most `KVM_MAX_MSR_ENTRIES`; the last chunk holds the
    /// deferred MSRs, which restore after all others.
    #[serde(with = "msr_chunks_serde")]
    pub saved_msrs: Vec<Msrs>,
    /// Debug registers.
    pub debug_regs: kvm_debugregs,
    /// LAPIC state.
    pub lapic: kvm_lapic_state,
    /// MP state.
    pub mp_state: kvm_mp_state,
    /// General purpose registers.
    pub regs: kvm_regs,
    /// Special registers.
    pub sregs: kvm_sregs,
    /// Pending interrupts, exceptions and NMIs.
    pub vcpu_events: kvm_vcpu_events,
    /// Extended control registers.
    pub xcrs: kvm_xcrs,
    /// XSAVE area (`KVM_GET_XSAVE2` when the host supports it).
    #[serde(with = "xsave_serde")]
    pub xsave: Xsave,
    /// TSC frequency in kHz, or `None` when the host did not report it.
    pub tsc_khz: Option<u32>,
}

// Hand-written because the FAM wrappers (`CpuId`, `Msrs`, `Xsave`) do not implement `PartialEq`:
// they are compared through their entries, and the XSAVE region word by word.
impl PartialEq for VcpuState {
    fn eq(&self, other: &Self) -> bool {
        let xsave_eq = |a: &Xsave, b: &Xsave| {
            a.as_fam_struct_ref().xsave.region == b.as_fam_struct_ref().xsave.region
                && a.as_slice() == b.as_slice()
        };
        self.cpuid.as_slice() == other.cpuid.as_slice()
            && self.saved_msrs.len() == other.saved_msrs.len()
            && self
                .saved_msrs
                .iter()
                .zip(&other.saved_msrs)
                .all(|(a, b)| a.as_slice() == b.as_slice())
            && self.debug_regs == other.debug_regs
            && self.lapic == other.lapic
            && self.mp_state == other.mp_state
            && self.regs == other.regs
            && self.sregs == other.sregs
            && self.vcpu_events == other.vcpu_events
            && self.xcrs == other.xcrs
            && xsave_eq(&self.xsave, &other.xsave)
            && self.tsc_khz == other.tsc_khz
    }
}

/// Serde adapters for the FAM wrappers in [`VcpuState`]. `vmm-sys-util` derives `Deserialize`
/// for its wrapper only when the wrapped struct has one, and the `kvm_xsave2` struct has none,
/// so each wrapper is stored as its plain entries and rebuilt on load.
mod cpuid_serde {
    use kvm_bindings::{CpuId, kvm_cpuid_entry2};
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(cpuid: &CpuId, s: S) -> Result<S::Ok, S::Error> {
        cpuid.as_slice().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<CpuId, D::Error> {
        let entries = Vec::<kvm_cpuid_entry2>::deserialize(d)?;
        CpuId::from_entries(&entries).map_err(D::Error::custom)
    }
}

mod msr_chunks_serde {
    use kvm_bindings::{Msrs, kvm_msr_entry};
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(chunks: &[Msrs], s: S) -> Result<S::Ok, S::Error> {
        chunks
            .iter()
            .map(Msrs::as_slice)
            .collect::<Vec<_>>()
            .serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Msrs>, D::Error> {
        Vec::<Vec<kvm_msr_entry>>::deserialize(d)?
            .iter()
            .map(|entries| Msrs::from_entries(entries).map_err(D::Error::custom))
            .collect()
    }
}

mod xsave_serde {
    use kvm_bindings::Xsave;
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// The XSAVE area as its fixed `kvm_xsave` region and its flexible array entries.
    #[derive(Serialize, Deserialize)]
    struct Parts {
        region: Vec<u32>,
        extra: Vec<u32>,
    }

    pub fn serialize<S: Serializer>(xsave: &Xsave, s: S) -> Result<S::Ok, S::Error> {
        Parts {
            region: xsave.as_fam_struct_ref().xsave.region.to_vec(),
            extra: xsave.as_slice().to_vec(),
        }
        .serialize(s)
    }

    #[allow(unsafe_code)]
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Xsave, D::Error> {
        let parts = Parts::deserialize(d)?;
        let region = <[u32; 1024]>::try_from(parts.region.as_slice()).map_err(|_| {
            D::Error::invalid_length(parts.region.len(), &"1024 XSAVE region words")
        })?;
        let mut xsave = Xsave::new(parts.extra.len()).map_err(D::Error::custom)?;
        // SAFETY: only the header's `region` is written. The `len` that sizes the flexible array
        // stays as `Xsave::new` set it, so the entries written below stay in bounds.
        unsafe { xsave.as_mut_fam_struct() }.xsave.region = region;
        xsave.as_mut_slice().copy_from_slice(&parts.extra);
        Ok(xsave)
    }
}

impl KvmVcpu {
    /// Saves this vCPU's state. The vCPU must be paused (out of `KVM_RUN`).
    pub fn save_state(&self) -> Result<VcpuState, KvmVcpuError> {
        // Ordering requirements (from upstream):
        //
        // KVM_GET_MP_STATE calls kvm_apic_accept_events(), which might modify
        // vCPU/LAPIC state. As such, it must be done before most everything
        // else, otherwise we cannot restore everything and expect it to work.
        //
        // KVM_GET_LAPIC may change state of LAPIC before returning it.
        //
        // KVM_GET_VCPU_EVENTS should be last: the GET ioctls above can modify
        // the internal state it reports.
        let mp_state = self
            .fd
            .get_mp_state()
            .map_err(KvmVcpuError::VcpuGetMpState)?;
        let regs = self.fd.get_regs().map_err(KvmVcpuError::VcpuGetRegs)?;
        let sregs = self.fd.get_sregs().map_err(KvmVcpuError::VcpuGetSregs)?;
        let xsave = self.get_xsave()?;
        let xcrs = self.fd.get_xcrs().map_err(KvmVcpuError::VcpuGetXcrs)?;
        let debug_regs = self
            .fd
            .get_debug_regs()
            .map_err(KvmVcpuError::VcpuGetDebugRegs)?;
        let lapic = self.fd.get_lapic().map_err(KvmVcpuError::VcpuGetLapic)?;
        let tsc_khz = match self.fd.get_tsc_khz() {
            Ok(khz) => Some(khz),
            Err(err) => {
                eprintln!(
                    "warning: TSC frequency not available ({err}); the snapshot cannot be loaded on a host with another TSC frequency"
                );
                None
            }
        };
        let cpuid = self.get_cpuid()?;
        let saved_msrs = self.get_msr_chunks(&self.msrs_to_save)?;
        let vcpu_events = self
            .fd
            .get_vcpu_events()
            .map_err(KvmVcpuError::VcpuGetVcpuEvents)?;

        Ok(VcpuState {
            cpuid,
            saved_msrs,
            debug_regs,
            lapic,
            mp_state,
            regs,
            sregs,
            vcpu_events,
            xcrs,
            xsave,
            tsc_khz,
        })
    }

    /// Restores a state [`KvmVcpu::save_state`] produced, before the vCPU first runs.
    pub fn restore_state(&self, state: &VcpuState) -> Result<(), KvmVcpuError> {
        // Ordering requirements (from upstream):
        //
        // Some SET ioctls (like set_mp_state) depend on kvm_vcpu_is_bsp(), so
        // the CPUID and the MP state are set before anything else.
        //
        // The TSC frequency is set before the MSRs, because the TSC MSRs are
        // interpreted at the vCPU's scaled frequency.
        //
        // SET_REGS clears pending exceptions unconditionally, thus, it must be
        // done before SET_VCPU_EVENTS, which restores it.
        //
        // SET_SREGS restores a pending interrupt and the APIC base, so it comes
        // before SET_LAPIC.
        //
        // SET_LAPIC must come before SET_MSRS, because the TSC deadline MSR
        // only restores successfully when the LAPIC is correctly configured.
        //
        // SET_VCPU_EVENTS comes last.
        self.fd
            .set_cpuid2(&state.cpuid)
            .map_err(KvmVcpuError::VcpuSetCpuid)?;
        if let Some(state_tsc_khz) = state.tsc_khz {
            self.scale_tsc_if_needed(state_tsc_khz)?;
        }
        self.fd
            .set_mp_state(state.mp_state)
            .map_err(KvmVcpuError::VcpuSetMpState)?;
        self.fd
            .set_regs(&state.regs)
            .map_err(KvmVcpuError::VcpuSetRegs)?;
        self.fd
            .set_sregs(&state.sregs)
            .map_err(KvmVcpuError::VcpuSetSregs)?;
        self.set_xsave(&state.xsave)?;
        self.fd
            .set_xcrs(&state.xcrs)
            .map_err(KvmVcpuError::VcpuSetXcrs)?;
        self.fd
            .set_debug_regs(&state.debug_regs)
            .map_err(KvmVcpuError::VcpuSetDebugRegs)?;
        self.fd
            .set_lapic(&state.lapic)
            .map_err(KvmVcpuError::VcpuSetLapic)?;
        for msrs in &state.saved_msrs {
            let nmsrs = self.fd.set_msrs(msrs).map_err(KvmVcpuError::VcpuSetMsrs)?;
            if nmsrs < msrs.as_slice().len() {
                return Err(KvmVcpuError::VcpuSetMsrsIncomplete);
            }
        }
        self.fd
            .set_vcpu_events(&state.vcpu_events)
            .map_err(KvmVcpuError::VcpuSetVcpuEvents)?;

        self.kvmclock_ctrl();
        Ok(())
    }

    /// Get the current XSAVE state for this vCPU.
    ///
    /// With `KVM_CAP_XSAVE2` the buffer is sized from the capability, so the dynamic XSTATE
    /// features (AMX tile data) that do not fit `kvm_xsave`'s fixed 4 KiB region are saved too.
    /// Without it, the fixed `KVM_GET_XSAVE` area is used.
    #[allow(unsafe_code)]
    fn get_xsave(&self) -> Result<Xsave, KvmVcpuError> {
        match self.xsave2_size {
            Some(xsave2_size) => {
                // The flexible array member holds the bytes past `kvm_xsave`'s region, in `u32`s.
                let fam_len = xsave2_size
                    .saturating_sub(std::mem::size_of::<kvm_xsave>())
                    .div_ceil(std::mem::size_of::<u32>());
                let mut xsave = Xsave::new(fam_len).map_err(KvmVcpuError::Fam)?;
                // SAFETY: `xsave` is allocated with the size KVM reported for XSAVE2, so
                // KVM_GET_XSAVE2 writes within the buffer.
                unsafe { self.fd.get_xsave2(&mut xsave) }.map_err(KvmVcpuError::VcpuGetXsave2)?;
                Ok(xsave)
            }
            None => {
                let xsave = self.fd.get_xsave().map_err(KvmVcpuError::VcpuGetXsave)?;
                // Without XSAVE2 there are no dynamic features past the fixed region, so the
                // wrapper has no flexible array entries.
                Xsave::from_header(kvm_xsave2 { len: 0, xsave }).map_err(KvmVcpuError::Fam)
            }
        }
    }

    /// Sets the XSAVE state for this vCPU. KVM validates the contents against the vCPU's XCR0
    /// and rejects an unusable snapshot with an error.
    #[allow(unsafe_code)]
    fn set_xsave(&self, xsave: &Xsave) -> Result<(), KvmVcpuError> {
        // SAFETY: `xsave` is a `Xsave` wrapper, so its buffer has the `kvm_xsave2` layout that
        // KVM_SET_XSAVE reads, with the entry count the wrapper records.
        unsafe { self.fd.set_xsave2(xsave) }.map_err(KvmVcpuError::VcpuSetXsave)
    }

    /// Scales this vCPU's TSC to the frequency a snapshot recorded, when the host's frequency
    /// differs by more than the tolerance in [`tsc_scaling_required`].
    fn scale_tsc_if_needed(&self, state_tsc_khz: u32) -> Result<(), KvmVcpuError> {
        let current_khz = self.fd.get_tsc_khz().map_err(KvmVcpuError::VcpuGetTsc)?;
        if tsc_scaling_required(current_khz, state_tsc_khz) {
            self.fd
                .set_tsc_khz(state_tsc_khz)
                .map_err(KvmVcpuError::VcpuSetTsc)?;
        }
        Ok(())
    }

    /// Calls KVM_KVMCLOCK_CTRL to avoid guest soft lockup watchdog panics on resume.
    /// See https://docs.kernel.org/virt/kvm/api.html .
    fn kvmclock_ctrl(&self) {
        // Failure is acceptable: EINVAL is returned when kvm-clock is not in use (for example
        // with the `no-kvmclock` guest kernel parameter), so it is only reported.
        if let Err(err) = self.fd.kvmclock_ctrl() {
            eprintln!("warning: KVM_KVMCLOCK_CTRL failed: {err}");
        }
    }

    /// Get CPUID for this vCPU.
    ///
    /// Opposed to KVM_GET_SUPPORTED_CPUID, KVM_GET_CPUID2 does not update "nent" with valid number
    /// of entries on success. Thus, when it passes "num_entries" greater than required, zeroed
    /// entries follow after valid entries. This function removes such zeroed empty entries.
    fn get_cpuid(&self) -> Result<CpuId, KvmVcpuError> {
        let mut cpuid = self
            .fd
            .get_cpuid2(KVM_MAX_CPUID_ENTRIES)
            .map_err(KvmVcpuError::VcpuGetCpuid)?;

        // As CPUID.0h:EAX should have the largest CPUID standard function, we don't need to check
        // EBX, ECX and EDX to confirm whether it is a valid entry.
        cpuid.retain(|entry| {
            !(entry.function == 0 && entry.index == 0 && entry.flags == 0 && entry.eax == 0)
        });

        Ok(cpuid)
    }

    /// Get the MSR values for the given indices, in chunks of at most `KVM_MAX_MSR_ENTRIES`
    /// (KVM reads a bounded number per call). The deferred MSRs are moved into a final chunk.
    fn get_msr_chunks(&self, msr_indices: &[u32]) -> Result<Vec<Msrs>, KvmVcpuError> {
        // + 1 for the chunk of deferred MSRs
        let mut msr_chunks: Vec<Msrs> =
            Vec::with_capacity(msr_indices.len().div_ceil(KVM_MAX_MSR_ENTRIES) + 1);
        for chunk in msr_indices.chunks(KVM_MAX_MSR_ENTRIES) {
            msr_chunks.push(self.get_msr_chunk(chunk)?);
        }

        fix_zero_tsc_deadline_msr(&mut msr_chunks);

        let deferred = extract_deferred_msrs(&mut msr_chunks).map_err(KvmVcpuError::Fam)?;
        msr_chunks.push(deferred);
        Ok(msr_chunks)
    }

    /// Gets one MSR chunk. KVM_GET_MSRS returns the number of MSRs it read; anything short of
    /// the whole chunk names the first MSR that failed.
    fn get_msr_chunk(&self, msr_indices: &[u32]) -> Result<Msrs, KvmVcpuError> {
        let mut msrs = Msrs::new(msr_indices.len()).map_err(KvmVcpuError::Fam)?;
        for (entry, index) in msrs.as_mut_slice().iter_mut().zip(msr_indices) {
            entry.index = *index;
        }

        let nmsrs = self
            .fd
            .get_msrs(&mut msrs)
            .map_err(KvmVcpuError::VcpuGetMsrs)?;
        if nmsrs != msr_indices.len() {
            let failed = msr_indices.get(nmsrs).copied().unwrap_or_default();
            return Err(KvmVcpuError::VcpuGetMsr(failed));
        }
        Ok(msrs)
    }
}

/// The MSR indices a snapshot saves: KVM's `KVM_GET_MSR_INDEX_LIST`, filtered to the
/// serializable ranges.
fn get_msrs_to_save(kvm: &kvm_ioctls::Kvm) -> Result<Vec<u32>, KvmVcpuError> {
    let msr_index_list = kvm
        .get_msr_index_list()
        .map_err(KvmVcpuError::GetMsrIndexList)?;
    Ok(msr_index_list
        .as_slice()
        .iter()
        .copied()
        .filter(|&index| msr_should_serialize(index))
        .collect())
}

/// The size of the `kvm_xsave2` area KVM reports for `KVM_CAP_XSAVE2`, or `None` when the
/// capability is absent (KVM returns 0 for it).
fn xsave2_size(kvm: &kvm_ioctls::Kvm) -> Option<usize> {
    usize::try_from(kvm.check_extension_int(Cap::Xsave2))
        .ok()
        .filter(|&size| size > 0)
}

/// A range of MSR indices (Firecracker's `MsrRange`).
struct MsrRange {
    base: u32,
    nmsrs: u32,
}

impl MsrRange {
    const fn new(base: u32, nmsrs: u32) -> Self {
        MsrRange { base, nmsrs }
    }

    /// Whether `msr` is in this range.
    fn contains(&self, msr: u32) -> bool {
        self.base <= msr && msr < self.base + self.nmsrs
    }
}

/// Base MSR for APIC
const APIC_BASE_MSR: u32 = 0x800;

/// Number of APIC MSR indexes
const APIC_MSR_INDEXES: u32 = 0x400;

/// Custom MSRs fall in the range 0x4b564d00-0x4b564dff
const MSR_KVM_RANGE_START: u32 = 0x4b56_4d00;
const MSR_KVM_RANGE_END: u32 = 0x4b56_4dff;

// List of MSRs that can be serialized (Firecracker's list). Sorted in ascending order of MSR
// addresses; the `MSR_IA32_MCG_CTL` exclusion is in [`msr_should_serialize`].
static SERIALIZABLE_MSR_RANGES: &[MsrRange] = &[
    MsrRange::new(MSR_IA32_P5_MC_ADDR, 1),
    MsrRange::new(MSR_IA32_P5_MC_TYPE, 1),
    MsrRange::new(MSR_IA32_TSC, 1),
    MsrRange::new(MSR_IA32_PLATFORM_ID, 1),
    MsrRange::new(MSR_IA32_APICBASE, 1),
    MsrRange::new(MSR_IA32_EBL_CR_POWERON, 1),
    MsrRange::new(MSR_EBC_FREQUENCY_ID, 1),
    MsrRange::new(MSR_SMI_COUNT, 1),
    MsrRange::new(MSR_IA32_FEAT_CTL, 1),
    MsrRange::new(MSR_IA32_TSC_ADJUST, 1),
    MsrRange::new(MSR_IA32_SPEC_CTRL, 1),
    MsrRange::new(MSR_IA32_PRED_CMD, 1),
    MsrRange::new(MSR_IA32_UCODE_WRITE, 1),
    MsrRange::new(MSR_IA32_UCODE_REV, 1),
    MsrRange::new(MSR_IA32_SMBASE, 1),
    MsrRange::new(MSR_FSB_FREQ, 1),
    MsrRange::new(MSR_PLATFORM_INFO, 1),
    MsrRange::new(MSR_PKG_CST_CONFIG_CONTROL, 1),
    MsrRange::new(MSR_IA32_MPERF, 1),
    MsrRange::new(MSR_IA32_APERF, 1),
    MsrRange::new(MSR_MTRR_CAP, 1),
    MsrRange::new(MSR_IA32_BBL_CR_CTL3, 1),
    MsrRange::new(MSR_IA32_SYSENTER_CS, 1),
    MsrRange::new(MSR_IA32_SYSENTER_ESP, 1),
    MsrRange::new(MSR_IA32_SYSENTER_EIP, 1),
    MsrRange::new(MSR_IA32_MCG_CAP, 1),
    MsrRange::new(MSR_IA32_MCG_STATUS, 1),
    MsrRange::new(MSR_IA32_MCG_CTL, 1),
    MsrRange::new(MSR_IA32_PERF_STATUS, 1),
    MsrRange::new(MSR_IA32_MISC_ENABLE, 1),
    MsrRange::new(MSR_MISC_FEATURE_CONTROL, 1),
    MsrRange::new(MSR_MISC_PWR_MGMT, 1),
    MsrRange::new(MSR_TURBO_RATIO_LIMIT, 1),
    MsrRange::new(MSR_TURBO_RATIO_LIMIT1, 1),
    MsrRange::new(MSR_IA32_DEBUGCTLMSR, 1),
    MsrRange::new(MSR_IA32_LASTBRANCHFROMIP, 1),
    MsrRange::new(MSR_IA32_LASTBRANCHTOIP, 1),
    MsrRange::new(MSR_IA32_LASTINTFROMIP, 1),
    MsrRange::new(MSR_IA32_LASTINTTOIP, 1),
    MsrRange::new(MSR_IA32_POWER_CTL, 1),
    // IA32_MTRR_PHYSBASE0 ..= the variable MTRR range
    MsrRange::new(0x200, 0x100),
    // MSR_CORE_C3_RESIDENCY, MSR_CORE_C6_RESIDENCY, MSR_CORE_C7_RESIDENCY
    MsrRange::new(MSR_CORE_C3_RESIDENCY, 3),
    MsrRange::new(MSR_IA32_MC0_CTL, 0x80),
    MsrRange::new(MSR_RAPL_POWER_UNIT, 1),
    // MSR_PKGC3_IRTL, MSR_PKGC6_IRTL, MSR_PKGC7_IRTL
    MsrRange::new(MSR_PKGC3_IRTL, 3),
    MsrRange::new(MSR_PKG_POWER_LIMIT, 1),
    MsrRange::new(MSR_PKG_ENERGY_STATUS, 1),
    MsrRange::new(MSR_PKG_PERF_STATUS, 1),
    MsrRange::new(MSR_PKG_POWER_INFO, 1),
    MsrRange::new(MSR_DRAM_POWER_LIMIT, 1),
    MsrRange::new(MSR_DRAM_ENERGY_STATUS, 1),
    MsrRange::new(MSR_DRAM_PERF_STATUS, 1),
    MsrRange::new(MSR_DRAM_POWER_INFO, 1),
    MsrRange::new(MSR_CONFIG_TDP_NOMINAL, 1),
    MsrRange::new(MSR_CONFIG_TDP_LEVEL_1, 1),
    MsrRange::new(MSR_CONFIG_TDP_LEVEL_2, 1),
    MsrRange::new(MSR_CONFIG_TDP_CONTROL, 1),
    MsrRange::new(MSR_TURBO_ACTIVATION_RATIO, 1),
    MsrRange::new(MSR_IA32_TSC_DEADLINE, 1),
    MsrRange::new(APIC_BASE_MSR, APIC_MSR_INDEXES),
    MsrRange::new(MSR_EFER, 1),
    MsrRange::new(MSR_STAR, 1),
    MsrRange::new(MSR_LSTAR, 1),
    MsrRange::new(MSR_CSTAR, 1),
    MsrRange::new(MSR_SYSCALL_MASK, 1),
    MsrRange::new(MSR_FS_BASE, 1),
    MsrRange::new(MSR_GS_BASE, 1),
    MsrRange::new(MSR_KERNEL_GS_BASE, 1),
    MsrRange::new(MSR_TSC_AUX, 1),
    MsrRange::new(MSR_MISC_FEATURES_ENABLES, 1),
    MsrRange::new(MSR_K7_HWCR, 1),
    MsrRange::new(MSR_IA32_TSX_CTRL, 1),
    // IA32_XSS controls which supervisor XSTATE components are enabled. Without it, the guest
    // kernel's XRSTORS of process FPU state containing CET/PT supervisor components triggers #GP.
    MsrRange::new(MSR_IA32_XSS, 1),
    MsrRange::new(
        MSR_KVM_RANGE_START,
        MSR_KVM_RANGE_END - MSR_KVM_RANGE_START + 1,
    ),
];

/// Specifies whether a particular MSR should be included in vcpu serialization.
///
/// # Arguments
///
/// * `index` - The index of the MSR that is checked whether it's needed for serialization.
fn msr_should_serialize(index: u32) -> bool {
    // Denied MSR not exported by Linux: IA32_MCG_CTL
    if index == MSR_IA32_MCG_CTL {
        return false;
    }
    SERIALIZABLE_MSR_RANGES
        .iter()
        .any(|range| range.contains(index))
}

/// Whether a host TSC at `current_khz` must be scaled to a snapshot's `state_khz`. Frequencies
/// within 250 ppm of each other count as equal: TSC calibration at boot commonly differs by
/// that much.
fn tsc_scaling_required(current_khz: u32, state_khz: u32) -> bool {
    let diff = (i64::from(current_khz) - i64::from(state_khz)).abs();
    // Cannot overflow since u32::MAX * 250 < i64::MAX
    diff > i64::from(state_khz) * TSC_KHZ_TOL_NUMERATOR / TSC_KHZ_TOL_DENOMINATOR
}

/// If the IA32_TSC_DEADLINE MSR value is zero, update it
/// with the IA32_TSC value to guarantee that
/// the vCPU will continue receiving interrupts after restoring from a snapshot.
///
/// Rationale: we observed that sometimes when taking a snapshot,
/// the IA32_TSC_DEADLINE MSR is cleared, but the interrupt is not
/// delivered to the guest, leading to a situation where one
/// of the vCPUs never receives TSC interrupts after restoring,
/// until the MSR is updated externally, eg by setting the system time.
fn fix_zero_tsc_deadline_msr(msr_chunks: &mut [Msrs]) {
    // We do not expect more than 1 TSC MSR entry, but if there are multiple, pick the maximum.
    let max_tsc_value = msr_chunks
        .iter()
        .flat_map(|msrs| msrs.as_slice())
        .filter(|msr| msr.index == MSR_IA32_TSC)
        .map(|msr| msr.data)
        .max();

    if let Some(tsc_value) = max_tsc_value {
        msr_chunks
            .iter_mut()
            .flat_map(|msrs| msrs.as_mut_slice())
            .filter(|msr| msr.index == MSR_IA32_TSC_DEADLINE && msr.data == 0)
            .for_each(|msr| {
                eprintln!("warning: MSR_IA32_TSC_DEADLINE is 0, replacing with {tsc_value:#x}");
                msr.data = tsc_value;
            });
    }
}

/// Looks for MSRs from the [`DEFERRED_MSRS`] array and removes them from `msr_chunks`.
/// Returns a new [`Msrs`] object containing all the removed MSRs.
///
/// We use this to capture some causal dependencies between MSRs where the relative order
/// of restoration matters (e.g. MSR_IA32_TSC must be restored before MSR_IA32_TSC_DEADLINE).
fn extract_deferred_msrs(msr_chunks: &mut [Msrs]) -> Result<Msrs, vmm_sys_util::fam::Error> {
    // Use 0 here as FamStructWrapper doesn't really give an equivalent of `Vec::with_capacity`,
    // and if we specify something N != 0 here, then it will create a FamStructWrapper with N
    // elements pre-allocated and zero'd out. Unless we then actually "fill" all those N values,
    // KVM will later yell at us about invalid MSRs.
    let mut deferred_msrs = Msrs::new(0)?;

    for msrs in msr_chunks.iter_mut() {
        msrs.retain(|msr| {
            if !DEFERRED_MSRS.contains(&msr.index) {
                return true;
            }
            match deferred_msrs.push(*msr) {
                Ok(()) => false,
                // Keep the MSR where it is when it cannot be moved.
                Err(err) => {
                    eprintln!(
                        "warning: failed to move MSR {:#x} into a later chunk: {err:?}",
                        msr.index
                    );
                    true
                }
            }
        });
    }

    Ok(deferred_msrs)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    // A raw CPUID entry shaped like KVM's leaf 1.
    fn leaf1() -> kvm_bindings::kvm_cpuid_entry2 {
        kvm_bindings::kvm_cpuid_entry2 {
            function: 1,
            index: 0,
            flags: 0,
            eax: 0,
            ebx: 0,
            ecx: 0,
            edx: 0,
            ..Default::default()
        }
    }

    // A raw CPUID entry shaped like KVM's leaf 0xB subleaf `index`.
    fn subleaf0b(index: u32) -> kvm_bindings::kvm_cpuid_entry2 {
        kvm_bindings::kvm_cpuid_entry2 {
            function: 0xB,
            index,
            flags: KVM_CPUID_FLAG_SIGNIFCANT_INDEX,
            eax: 0xdead,
            ebx: 0xbeef,
            ecx: 0xcafe,
            edx: 0x1234_5678,
            ..Default::default()
        }
    }

    #[test]
    fn test_normalize_cpuid_topology_leaf1() {
        let mut entries = vec![leaf1(), subleaf0b(0), subleaf0b(1)];
        normalize_cpuid_topology(&mut entries, 3, 6, 0);

        let leaf1 = &entries[0];
        // Initial APIC ID: EBX[31:24] = 3.
        assert_eq!((leaf1.ebx >> 24) & 0xff, 3);
        // Max addressable logical processor IDs: next power of two >= 6 = 8.
        assert_eq!((leaf1.ebx >> 16) & 0xff, 8);
        // HTT set with more than one vCPU; TSC-Deadline and Hypervisor set.
        assert_ne!(leaf1.edx & (1 << 28), 0);
        assert_ne!(leaf1.ecx & (1 << 24), 0);
        assert_ne!(leaf1.ecx & (1 << 31), 0);

        // Single-vCPU topology: no HTT.
        normalize_cpuid_topology(&mut entries, 0, 1, 0);
        assert_eq!(entries[0].edx & (1 << 28), 0);
        assert_eq!((entries[0].ebx >> 16) & 0xff, 1);
    }

    #[test]
    fn test_normalize_cpuid_topology_leaf0b() {
        let mut entries = vec![leaf1(), subleaf0b(0), subleaf0b(1)];
        normalize_cpuid_topology(&mut entries, 3, 6, 1);

        // Subleaf 0: logical processor domain, SMT with 2 logical processors.
        let sub0 = &entries[1];
        assert_eq!(sub0.eax & 0x1f, 1);
        assert_eq!(sub0.ebx & 0xffff, 2);
        assert_eq!((sub0.ecx >> 8) & 0xff, 1);
        assert_eq!(sub0.edx, 3);

        // Subleaf 1: core domain; 2^N >= MAX_SUPPORTED_VCPUS (N = 5 for 32),
        // all 6 logical processors, domain type 2.
        let sub1 = &entries[2];
        assert_eq!(
            sub1.eax & 0x1f,
            u32::from(MAX_SUPPORTED_VCPUS.next_power_of_two().ilog2())
        );
        assert_eq!(sub1.ebx & 0xffff, 6);
        assert_eq!(sub1.ecx & 0xff, 1);
        assert_eq!((sub1.ecx >> 8) & 0xff, 2);
        assert_eq!(sub1.edx, 3);
    }

    #[test]
    fn test_set_bit_and_range() {
        let mut value = 0u32;
        set_bit(&mut value, 28, true);
        assert_eq!(value, 1 << 28);
        set_bit(&mut value, 28, false);
        assert_eq!(value, 0);

        let mut value = 0u32;
        set_range(&mut value, 24..=31, 0xab);
        assert_eq!(value, 0xab << 24);
        set_range(&mut value, 16..=23, 5);
        assert_eq!(value, (0xab << 24) | (5 << 16));
    }

    #[test]
    fn test_msr_should_serialize() {
        assert!(msr_should_serialize(MSR_IA32_TSC));
        assert!(msr_should_serialize(MSR_EFER));
        assert!(msr_should_serialize(APIC_BASE_MSR + 0x3ff));
        assert!(msr_should_serialize(MSR_KVM_RANGE_START));
        // Denied explicitly.
        assert!(!msr_should_serialize(MSR_IA32_MCG_CTL));
        // Hyper-V MSRs are not serialized.
        assert!(!msr_should_serialize(0x4000_0000));
    }

    #[test]
    fn test_tsc_scaling_required() {
        assert!(!tsc_scaling_required(2_000_000, 2_000_000));
        // The tolerance is 250 ppm of 2 GHz, i.e. 500 kHz: 100 kHz is within it, 10 MHz is not.
        assert!(!tsc_scaling_required(2_000_100, 2_000_000));
        assert!(tsc_scaling_required(2_010_000, 2_000_000));
    }

    #[test]
    fn test_extract_deferred_msrs_moves_tsc_deadline_last() {
        let entry = |index, data| kvm_bindings::kvm_msr_entry {
            index,
            data,
            ..Default::default()
        };
        let mut chunks = vec![
            Msrs::from_entries(&[entry(MSR_IA32_TSC_DEADLINE, 7), entry(MSR_IA32_TSC, 9)]).unwrap(),
        ];

        let deferred = extract_deferred_msrs(&mut chunks).unwrap();
        assert_eq!(deferred.as_slice().len(), 1);
        assert_eq!(deferred.as_slice()[0].index, MSR_IA32_TSC_DEADLINE);
        let remaining: Vec<u32> = chunks[0].as_slice().iter().map(|msr| msr.index).collect();
        assert_eq!(remaining, vec![MSR_IA32_TSC]);
    }

    // A VM with one vCPU, created the way the boot path creates it (irqchip before the vCPU).
    // Needs `/dev/kvm`.
    fn vm_with_vcpu() -> (KvmVm, KvmVcpu) {
        let mut vm = KvmVm::new(Kvm::new().unwrap()).unwrap();
        vm.arch_pre_create_vcpus(1).unwrap();
        let vcpu = KvmVcpu::new(0, &vm).unwrap();
        (vm, vcpu)
    }

    // The saved MSR values, flattened. MSR_IA32_TSC and MSR_IA32_TSC_DEADLINE follow host time on a
    // vCPU that never ran: a deadline that has already passed reads back as 0 and is then
    // replaced with the current TSC, so both move between the two saves and are left out.
    fn msr_values(state: &VcpuState) -> BTreeMap<u32, u64> {
        state
            .saved_msrs
            .iter()
            .flat_map(|msrs| msrs.as_slice())
            .filter(|msr| msr.index != MSR_IA32_TSC && msr.index != MSR_IA32_TSC_DEADLINE)
            .map(|msr| (msr.index, msr.data))
            .collect()
    }

    #[test]
    fn test_save_restore_round_trip() {
        if !crate::vstate::kvm::usable_for_tests() {
            eprintln!("skipped: /dev/kvm is not usable here");
            return;
        }
        let (vm, mut vcpu) = vm_with_vcpu();
        vcpu.configure_cpuid(vm.kvm(), 1, false).unwrap();
        vcpu.configure_msrs_for_boot().unwrap();
        vcpu.fd
            .set_regs(&kvm_regs {
                rip: 0x1000,
                rax: 0xdead_beef,
                rflags: 0x2,
                ..Default::default()
            })
            .unwrap();
        let mut sregs = vcpu.fd.get_sregs().unwrap();
        sregs.cr3 = 0x5000;
        vcpu.fd.set_sregs(&sregs).unwrap();

        let state = vcpu.save_state().unwrap();
        assert!(!state.saved_msrs.is_empty());

        // A second VM with a fresh vCPU takes the state.
        let (_vm2, restored) = vm_with_vcpu();
        restored.restore_state(&state).unwrap();
        let again = restored.save_state().unwrap();

        assert_eq!(again.regs, state.regs);
        assert_eq!(again.regs.rip, 0x1000);
        assert_eq!(again.sregs, state.sregs);
        assert_eq!(again.sregs.cr3, 0x5000);
        assert_eq!(again.cpuid.as_slice(), state.cpuid.as_slice());
        assert_eq!(msr_values(&again), msr_values(&state));
    }
}
