// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port. Firecracker has no Hypervisor.framework backend;
// Hypervisor.framework has no in-kernel interrupt controller for a VMM that
// does not create one, so the exit handling below (which exits advance the
// guest's program counter, how a `WFI` is idled, how the virtual timer is
// mirrored into the controller, and how a device access is emulated from the
// instruction syndrome) follows the decisions of the reference HVF VMM - QEMU's
// `target/arm/hvf/hvf.c`, GPL-2.0 - read for its protocol, not copied.

//! The VM and vCPUs of the Hypervisor.framework backend.
//!
//! # One VM per process
//!
//! `hv_vm_create` creates *the* VM of the calling process: Hypervisor.framework
//! supports one VM per process, so booting several microVMs on Apple Silicon
//! means running one VMM process per VM (the KVM backends have no such limit).
//! A second attempt fails with `HV_BUSY`, and [`HvfVm::create`] says so in its
//! error.
//!
//! # Threads
//!
//! A vCPU belongs to the thread that created it: `hv_vcpu_create`,
//! `hv_vcpu_run`, every register access and `hv_vcpu_destroy` must happen on
//! that thread, so each vCPU runs on its own thread and [`HvfVcpu`] is only
//! ever touched by it. The only cross-thread call is `hv_vcpus_exit`, which
//! asks a vCPU to leave `hv_vcpu_run` (`HV_EXIT_REASON_CANCELED`).

// FFI boundary (see the allowlist in `lib.rs`): `hv_vm_create`/`map`/`unmap`/
// `destroy` — the memory mapping calls that make guest RAM visible to
// Hypervisor.framework.
#![allow(unsafe_code)]

use std::ffi::c_void;

use vm_memory::GuestMemoryBackend;

use crate::vstate::memory::{Address, GuestMemoryMmap, GuestMemoryRegion, host_page_size};

use super::sys::{self, HvfError};

/// The VM of this process, and the guest memory mapped into it.
#[derive(Debug)]
pub struct HvfVm {
    /// The regions mapped into the guest, so that they can be unmapped and
    /// the VM destroyed in a defined order.
    mapped: Vec<(u64, usize)>,
}

impl HvfVm {
    /// Creates the VM of this process.
    ///
    /// Fails with an error naming `HV_BUSY` when this process already has a
    /// VM: Hypervisor.framework allows exactly one per process.
    pub fn create() -> Result<Self, HvfError> {
        sys::hv_try!(hv_vm_create(sys::NO_VM_CONFIG))?;
        Ok(Self { mapped: Vec::new() })
    }

    /// Maps every region of `memory` into the guest physical address space.
    pub fn map_guest_memory(&mut self, memory: &GuestMemoryMmap) -> Result<(), HvfError> {
        for region in memory.iter() {
            let ipa = region.start_addr().raw_value();
            let len = usize::try_from(region.len()).map_err(|_| HvfError::Mapping {
                ipa,
                reason: "the region is larger than the host's address space",
            })?;
            self.map(region.as_ptr(), ipa, len)?;
        }
        Ok(())
    }

    /// Maps one region, checking the alignment Hypervisor.framework requires
    /// before calling it rather than after a crash.
    ///
    /// `hv_vm_map` takes this process's own memory for the given guest
    /// physical range; both addresses and the size have to be multiples of
    /// the *host* page size, which is 16 KiB on Apple Silicon - not the 4 KiB
    /// a guest believes in. The guest memory this port allocates is 2 MiB
    /// aligned and sized in whole MiB, so this holds, but a wrong host page
    /// assumption would otherwise fault on hardware rather than here.
    fn map(&mut self, host: *const u8, ipa: u64, len: usize) -> Result<(), HvfError> {
        let page = host_page_size();
        let aligned = |value: usize| value.is_multiple_of(page);
        if !aligned(host as usize) {
            return Err(HvfError::Mapping {
                ipa,
                reason: "the host address is not aligned to the host page size",
            });
        }
        if !aligned(ipa as usize) {
            return Err(HvfError::Mapping {
                ipa,
                reason: "the guest address is not aligned to the host page size",
            });
        }
        if len == 0 || !aligned(len) {
            return Err(HvfError::Mapping {
                ipa,
                reason: "the size is zero or not a multiple of the host page size",
            });
        }
        let flags = sys::HV_MEMORY_READ | sys::HV_MEMORY_WRITE | sys::HV_MEMORY_EXEC;
        sys::hv_try!(hv_vm_map(host as *mut c_void, ipa, len, flags))?;
        self.mapped.push((ipa, len));
        Ok(())
    }
}

impl Drop for HvfVm {
    fn drop(&mut self) {
        // The VM is destroyed from whichever thread drops the last reference
        // to it - in practice the last vCPU thread to finish, since that is
        // the last holder. Unlike the vCPU calls, `hv_vm_destroy` is not
        // thread-bound; and because Hypervisor.framework allows one VM per
        // process, the process teardown that follows a stopped VM would
        // release it anyway. The unmapping is explicit so that the guest's
        // pages are released before the memory itself is freed, which is what
        // the field order of `SharedVm` arranges.
        for (ipa, len) in self.mapped.drain(..) {
            let ret = unsafe { sys::hv_vm_unmap(ipa, len) };
            if ret != sys::HV_SUCCESS {
                eprintln!("warning: hv_vm_unmap({ipa:#x}) failed: {ret:#010x}");
            }
        }
        let ret = unsafe { sys::hv_vm_destroy() };
        if ret != sys::HV_SUCCESS {
            eprintln!("warning: hv_vm_destroy failed: {ret:#010x}");
        }
    }
}
