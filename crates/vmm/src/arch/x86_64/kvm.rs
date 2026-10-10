// Copyright 2025 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/arch/x86_64/kvm.rs (trimmed: the
// `msrs_to_save` snapshot helper is not ported; CPU-template capability
// modifiers are dropped along with `cpu_config`).

use kvm_bindings::{CpuId, KVM_MAX_CPUID_ENTRIES};

use crate::arch::x86_64::xstate::{XstateError, request_dynamic_xstate_features};

/// Architecture specific error for KVM initialization
#[derive(Debug, thiserror::Error)]
pub enum KvmArchError {
    /// Failed to get supported cpuid: {0}
    #[error("Failed to get supported cpuid: {0}")]
    GetSupportedCpuId(kvm_ioctls::Error),
    /// Failed to request permission for dynamic XSTATE features: {0}
    #[error("Failed to request permission for dynamic XSTATE features: {0}")]
    XstateFeatures(XstateError),
}

/// Struct with kvm fd and kvm associated parameters.
#[derive(Debug)]
pub struct Kvm {
    /// KVM fd.
    pub fd: kvm_ioctls::Kvm,
    /// Supported CpuIds.
    pub supported_cpuid: CpuId,
}

impl Kvm {
    #[allow(missing_docs)]
    pub(crate) const DEFAULT_CAPABILITIES: [u32; 14] = [
        kvm_bindings::KVM_CAP_IRQCHIP,
        kvm_bindings::KVM_CAP_IOEVENTFD,
        kvm_bindings::KVM_CAP_IRQFD,
        kvm_bindings::KVM_CAP_USER_MEMORY,
        kvm_bindings::KVM_CAP_SET_TSS_ADDR,
        kvm_bindings::KVM_CAP_PIT2,
        kvm_bindings::KVM_CAP_PIT_STATE2,
        kvm_bindings::KVM_CAP_ADJUST_CLOCK,
        kvm_bindings::KVM_CAP_DEBUGREGS,
        kvm_bindings::KVM_CAP_MP_STATE,
        kvm_bindings::KVM_CAP_VCPU_EVENTS,
        kvm_bindings::KVM_CAP_XCRS,
        kvm_bindings::KVM_CAP_XSAVE,
        kvm_bindings::KVM_CAP_EXT_CPUID,
    ];

    /// Initialize [`Kvm`] type for x86_64 architecture
    pub fn init_arch(fd: kvm_ioctls::Kvm) -> Result<Self, KvmArchError> {
        request_dynamic_xstate_features().map_err(KvmArchError::XstateFeatures)?;

        let supported_cpuid = fd
            .get_supported_cpuid(KVM_MAX_CPUID_ENTRIES)
            .map_err(KvmArchError::GetSupportedCpuId)?;

        Ok(Kvm {
            fd,
            supported_cpuid,
        })
    }
}
