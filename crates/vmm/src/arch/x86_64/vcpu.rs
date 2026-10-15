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
// snapshot save/restore, TSC-offset and MSR-dump helpers are dropped; only
// `/dev/kvm`-free tests are kept).

use std::sync::Arc;

use kvm_bindings::KVM_CPUID_FLAG_SIGNIFCANT_INDEX;
use kvm_ioctls::{VcpuExit, VcpuFd};

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

/// Errors associated with the wrappers over KVM ioctls.
#[derive(Debug, thiserror::Error)]
pub enum KvmVcpuError {
    /// Cannot open the VCPU file descriptor: {0}
    #[error("Cannot open the VCPU file descriptor: {0}")]
    VcpuFd(kvm_ioctls::Error),
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

        Ok(KvmVcpu {
            index,
            fd: kvm_vcpu,
            peripherals: Default::default(),
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
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VcpuState {}

impl KvmVcpu {
    /// Saves this vCPU's state. The vCPU must be paused (out of `KVM_RUN`).
    pub fn save_state(&self) -> Result<VcpuState, KvmVcpuError> {
        todo!("x86_64 vCPU save")
    }

    /// Restores a state [`KvmVcpu::save_state`] produced, before the vCPU first runs.
    pub fn restore_state(&self, state: &VcpuState) -> Result<(), KvmVcpuError> {
        let _ = state;
        todo!("x86_64 vCPU restore")
    }
}

#[cfg(test)]
mod tests {
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
}
