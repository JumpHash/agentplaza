// Copyright 2019 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/arch/x86_64/msr.rs (trimmed: the
// serializable/dumpable MSR range lists feed snapshots and CPU-config dumps,
// which are not ported; only the Linux boot MSR setup remains).

/// Model Specific Registers (MSRs) related functionality.
use kvm_bindings::{Msrs, kvm_msr_entry};
use kvm_ioctls::VcpuFd;

use crate::arch::x86_64::generated::msr_index::*;

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
/// MSR related errors.
pub enum MsrError {
    /// Failed to create `vmm_sys_util::fam::FamStructWrapper` for MSRs
    #[error("Failed to create `vmm_sys_util::fam::FamStructWrapper` for MSRs")]
    Fam(#[from] vmm_sys_util::fam::Error),
    /// Failed to set MSRs: {0}
    #[error("Failed to set MSRs: {0}")]
    SetMsrs(kvm_ioctls::Error),
    /// Not all given MSRs were set.
    #[error("Not all given MSRs were set.")]
    SetMsrsIncomplete,
}

/// Creates and populates required MSR entries for booting Linux on X86_64.
pub fn create_boot_msr_entries() -> Vec<kvm_msr_entry> {
    let msr_entry_default = |msr| kvm_msr_entry {
        index: msr,
        data: 0x0,
        ..Default::default()
    };

    vec![
        msr_entry_default(MSR_IA32_SYSENTER_CS),
        msr_entry_default(MSR_IA32_SYSENTER_ESP),
        msr_entry_default(MSR_IA32_SYSENTER_EIP),
        // x86_64 specific msrs, we only run on x86_64 not x86.
        msr_entry_default(MSR_STAR),
        msr_entry_default(MSR_CSTAR),
        msr_entry_default(MSR_KERNEL_GS_BASE),
        msr_entry_default(MSR_SYSCALL_MASK),
        msr_entry_default(MSR_LSTAR),
        // end of x86_64 specific code
        msr_entry_default(MSR_IA32_TSC),
        kvm_msr_entry {
            index: MSR_IA32_MISC_ENABLE,
            data: u64::from(MSR_IA32_MISC_ENABLE_FAST_STRING),
            ..Default::default()
        },
        // set default memory type for physical memory outside configured
        // memory ranges to write-back by setting MTRR enable bit (11) and
        // setting memory type to write-back (value 6).
        // https://wiki.osdev.org/MTRR
        kvm_msr_entry {
            index: MSR_MTRRdefType,
            data: (1 << 11) | 0x6,
            ..Default::default()
        },
    ]
}

/// Configure Model Specific Registers (MSRs) required to boot Linux for a given x86_64 vCPU.
///
/// # Arguments
///
/// * `vcpu` - Structure for the VCPU that holds the VCPU's fd.
///
/// # Errors
///
/// When:
/// - Failed to create [`vmm_sys_util::fam::FamStructWrapper`] for MSRs.
/// - [`kvm_ioctls::ioctls::vcpu::VcpuFd::set_msrs`] errors.
/// - [`kvm_ioctls::ioctls::vcpu::VcpuFd::set_msrs`] fails to write all given MSRs entries.
pub fn set_msrs(vcpu: &VcpuFd, msr_entries: &[kvm_msr_entry]) -> Result<(), MsrError> {
    let msrs = Msrs::from_entries(msr_entries)?;
    vcpu.set_msrs(&msrs)
        .map_err(MsrError::SetMsrs)
        .and_then(|msrs_written| {
            if msrs_written == msrs.as_fam_struct_ref().nmsrs as usize {
                Ok(())
            } else {
                Err(MsrError::SetMsrsIncomplete)
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_boot_msr_entries_are_distinct() {
        // The boot MSR entries are written with a single KVM_SET_MSRS call,
        // which silently ignores duplicate indices; distinct indices keep the
        // full boot configuration effective.
        let mut seen = std::collections::HashSet::new();
        for entry in create_boot_msr_entries() {
            assert!(
                seen.insert(entry.index),
                "duplicate boot MSR {:#x}",
                entry.index
            );
        }
    }
}
