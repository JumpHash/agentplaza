// Copyright 2025 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/arch/aarch64/kvm.rs (trimmed: the
// `kvm_cap_modifiers` CPU-template capability list and the snapshot
// `KvmState` are dropped; `init_arch` therefore takes only the KVM fd).

use std::convert::Infallible;

use kvm_ioctls::Kvm as KvmFd;

/// ['Kvm'] initialization can't fail for Aarch64
pub type KvmArchError = Infallible;

/// Optional capabilities.
#[derive(Debug, Default)]
pub struct OptionalCapabilities {
    /// KVM_CAP_COUNTER_OFFSET
    pub counter_offset: bool,
}

/// Struct with kvm fd and kvm associated parameters.
#[derive(Debug)]
pub struct Kvm {
    /// KVM fd.
    pub fd: KvmFd,
}

impl Kvm {
    pub(crate) const DEFAULT_CAPABILITIES: [u32; 7] = [
        kvm_bindings::KVM_CAP_IOEVENTFD,
        kvm_bindings::KVM_CAP_IRQFD,
        kvm_bindings::KVM_CAP_USER_MEMORY,
        kvm_bindings::KVM_CAP_ARM_PSCI_0_2,
        kvm_bindings::KVM_CAP_DEVICE_CTRL,
        kvm_bindings::KVM_CAP_MP_STATE,
        kvm_bindings::KVM_CAP_ONE_REG,
    ];

    /// Initialize [`Kvm`] type for Aarch64 architecture
    pub fn init_arch(fd: KvmFd) -> Result<Self, KvmArchError> {
        Ok(Self { fd })
    }

    /// Returns struct with optional capabilities statuses.
    pub fn optional_capabilities(&self) -> OptionalCapabilities {
        OptionalCapabilities {
            counter_offset: self
                .fd
                .check_extension_raw(kvm_bindings::KVM_CAP_COUNTER_OFFSET.into())
                != 0,
        }
    }
}
