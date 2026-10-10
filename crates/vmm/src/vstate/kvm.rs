// Copyright 2020 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/vstate/kvm.rs (trimmed: CPU-template
// capability modifiers and the snapshot `KvmState` are dropped; the arch
// re-export is cfg-selected because this tree's `arch/mod.rs` declares the
// per-arch modules rather than re-exporting their items).

use kvm_bindings::KVM_API_VERSION;
use kvm_ioctls::Kvm as KvmFd;

// Upstream re-exports the arch-specific `Kvm` through `crate::arch`; select
// the current arch's module here. The aarch64 arm lands with the aarch64 port.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub use crate::arch::x86_64::kvm::{Kvm, KvmArchError};

/// Errors associated with the wrappers over KVM ioctls.
#[derive(Debug, thiserror::Error)]
pub enum KvmError {
    /// The host kernel reports an invalid KVM API version: {0}
    #[error("The host kernel reports an invalid KVM API version: {0}")]
    ApiVersion(i32),
    /// Missing KVM capabilities: {0:#x?}
    #[error("Missing KVM capabilities: {0:#x?}")]
    Capabilities(u32),
    /// Error creating KVM object: {0}
    #[error("Error creating KVM object: {0}")]
    Kvm(#[from] kvm_ioctls::Error),
    /// Architecture specific error: {0}
    #[error("Architecture specific error: {0}")]
    ArchError(#[from] KvmArchError),
}

impl Kvm {
    /// Create `Kvm` struct.
    pub fn new() -> Result<Self, KvmError> {
        let kvm_fd = KvmFd::new().map_err(KvmError::Kvm)?;

        // Check that KVM has the correct version.
        // Safe to cast because this is a constant.
        #[allow(clippy::cast_possible_wrap)]
        if kvm_fd.get_api_version() != KVM_API_VERSION as i32 {
            return Err(KvmError::ApiVersion(kvm_fd.get_api_version()));
        }

        // Check that all desired capabilities are supported.
        Self::check_capabilities(&kvm_fd, &Self::DEFAULT_CAPABILITIES).map_err(KvmError::Capabilities)?;

        Kvm::init_arch(kvm_fd).map_err(KvmError::ArchError)
    }

    fn check_capabilities(kvm_fd: &KvmFd, capabilities: &[u32]) -> Result<(), u32> {
        for cap in capabilities {
            // If capability is not supported kernel will return 0.
            if kvm_fd.check_extension_raw(u64::from(*cap)) == 0 {
                return Err(*cap);
            }
        }
        Ok(())
    }

    /// Returns the maximal number of memslots allowed in a [`KvmVm`]
    pub fn max_nr_memslots(&self) -> u32 {
        self.fd
            .get_nr_memslots()
            .try_into()
            .expect("Number of vcpus reported by KVM exceeds u32::MAX")
    }
}
