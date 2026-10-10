// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port. The declarations below mirror Apple's
// `Hypervisor/Hypervisor.h` and the three `hv_*` headers for arm64; the
// signatures, structure layouts and constant values were checked against the
// header-derived bindings of the `ahv` crate (MIT OR Apache-2.0), which is a
// faithful mirror of the system header, rather than typed from memory - a
// wrong structure layout here is an immediate crash on real hardware.

//! The Hypervisor.framework C API, for `aarch64` macOS.
//!
//! Only what this backend uses is declared: VM creation and memory mapping,
//! vCPU creation, register and system-register access, interrupt injection,
//! the virtual timer, and the exit structure `hv_vcpu_run` fills in.
//!
//! # Dynamic library and deployment target
//!
//! `hv_vcpu_config_create` and `hv_vm_config_get_default_ipa_size` exist from
//! macOS 13; the backend therefore targets macOS 13.0 or later. The process
//! also needs the `com.apple.security.hypervisor` entitlement: a binary
//! without it fails `hv_vm_create` with `HV_DENIED`.

#![allow(non_camel_case_types)]

use std::ffi::c_void;
use std::ptr;

/// A status code returned by every Hypervisor.framework call.
pub type hv_return_t = u32;
/// An opaque vCPU handle. Valid only on the thread that created it.
pub type hv_vcpu_t = u64;
/// An intermediate physical address (a guest physical address).
pub type hv_ipa_t = u64;
/// A general-purpose register identifier.
pub type hv_reg_t = u32;
/// A system register identifier, in the encoding of [`crate::hvf::exit::sysreg`].
pub type hv_sys_reg_t = u16;
/// An interrupt type (`HV_INTERRUPT_TYPE_*`).
pub type hv_interrupt_type_t = u32;
/// Memory region permissions (`HV_MEMORY_*`).
pub type hv_memory_flags_t = u64;
/// An opaque VM configuration object (macOS 13 and later).
pub type hv_vm_config_t = *mut c_void;
/// An opaque vCPU configuration object (macOS 13 and later).
pub type hv_vcpu_config_t = *mut c_void;
/// A vCPU feature register identifier (`HV_FEATURE_REG_*`).
pub type hv_feature_reg_t = u32;

/// `HV_SUCCESS`: the call succeeded.
pub const HV_SUCCESS: hv_return_t = 0;
/// `HV_ERROR`: the call failed.
pub const HV_ERROR: hv_return_t = 0xfae9_4001;
/// `HV_BUSY`: the operation cannot happen while the object is in its current
/// state - which is what a second `hv_vm_create` in one process returns.
pub const HV_BUSY: hv_return_t = 0xfae9_4002;
/// `HV_BAD_ARGUMENT`: an argument was out of range.
pub const HV_BAD_ARGUMENT: hv_return_t = 0xfae9_4003;
/// `HV_ILLEGAL_GUEST_STATE`: the guest state is inconsistent; running the vCPU
/// again would not help.
pub const HV_ILLEGAL_GUEST_STATE: hv_return_t = 0xfae9_4004;
/// `HV_NO_RESOURCES`: out of a resource (memory, vCPUs).
pub const HV_NO_RESOURCES: hv_return_t = 0xfae9_4005;
/// `HV_NO_DEVICE`: the device handle is invalid.
pub const HV_NO_DEVICE: hv_return_t = 0xfae9_4006;
/// `HV_DENIED`: the process lacks the entitlement, or the operation is not
/// permitted.
pub const HV_DENIED: hv_return_t = 0xfae9_4007;
/// `HV_FAULT`: the operation faulted.
pub const HV_FAULT: hv_return_t = 0xfae9_4008;
/// `HV_UNSUPPORTED`: the feature is unsupported on this host.
pub const HV_UNSUPPORTED: hv_return_t = 0xfae9_400f;

/// `HV_EXIT_REASON_CANCELED`: the vCPU was asked to exit by
/// `hv_vcpus_exit`.
pub const HV_EXIT_REASON_CANCELED: u32 = 0;
/// `HV_EXIT_REASON_EXCEPTION`: the guest took a trap; the exception
/// information is valid.
pub const HV_EXIT_REASON_EXCEPTION: u32 = 1;
/// `HV_EXIT_REASON_VTIMER_ACTIVATED`: the guest's virtual timer became
/// pending, and Hypervisor.framework masked it until the VM unmasks it.
pub const HV_EXIT_REASON_VTIMER_ACTIVATED: u32 = 2;
/// `HV_EXIT_REASON_UNKNOWN`: an exit the framework does not document.
pub const HV_EXIT_REASON_UNKNOWN: u32 = 3;

/// `HV_INTERRUPT_TYPE_IRQ`.
pub const HV_INTERRUPT_TYPE_IRQ: hv_interrupt_type_t = 0;
/// `HV_MEMORY_READ`.
pub const HV_MEMORY_READ: hv_memory_flags_t = 1 << 0;
/// `HV_MEMORY_WRITE`.
pub const HV_MEMORY_WRITE: hv_memory_flags_t = 1 << 1;
/// `HV_MEMORY_EXEC`.
pub const HV_MEMORY_EXEC: hv_memory_flags_t = 1 << 2;

// General-purpose registers. `HV_REG_PC` is 31, `FPCR` 32, `FPSR` 33 and
// `CPSR` (PSTATE) 34; X0..X30 are 0..30.
/// `HV_REG_X0` .. `HV_REG_X30`: pass X0 through X30 as `hv_reg_t`.
pub const HV_REG_X0: hv_reg_t = 0;
/// `HV_REG_PC`.
pub const HV_REG_PC: hv_reg_t = 31;
/// `HV_REG_CPSR`: PSTATE.
pub const HV_REG_CPSR: hv_reg_t = 34;

// System registers, in the encoding of `exit::sysreg`. Each value is asserted
// against the system header's by the tests below.
use crate::hvf::exit::sysreg;

macro_rules! sys_reg {
    ($name:ident, $op0:expr, $op1:expr, $crn:expr, $crm:expr, $op2:expr) => {
        #[doc = concat!("`", stringify!($name), "`.")]
        pub const $name: hv_sys_reg_t = sysreg($op0, $op1, $crn, $crm, $op2);
    };
}

sys_reg!(HV_SYS_REG_MPIDR_EL1, 3, 0, 0, 0, 5);
sys_reg!(HV_SYS_REG_ID_AA64PFR0_EL1, 3, 0, 0, 4, 0);
sys_reg!(HV_SYS_REG_ID_AA64MMFR0_EL1, 3, 0, 0, 7, 0);
sys_reg!(HV_SYS_REG_SCTLR_EL1, 3, 0, 1, 0, 0);
sys_reg!(HV_SYS_REG_SPSR_EL1, 3, 0, 4, 0, 0);
sys_reg!(HV_SYS_REG_ELR_EL1, 3, 0, 4, 0, 1);
sys_reg!(HV_SYS_REG_ESR_EL1, 3, 0, 5, 2, 0);
sys_reg!(HV_SYS_REG_FAR_EL1, 3, 0, 6, 0, 0);
sys_reg!(HV_SYS_REG_VBAR_EL1, 3, 0, 12, 0, 0);
sys_reg!(HV_SYS_REG_CNTV_CTL_EL0, 3, 3, 14, 3, 1);
sys_reg!(HV_SYS_REG_CNTV_CVAL_EL0, 3, 3, 14, 3, 2);

/// The feature register holding ID_AA64PFR0_EL1, for
/// [`hv_vcpu_config_get_feature_reg`].
pub const HV_FEATURE_REG_ID_AA64PFR0_EL1: hv_feature_reg_t = 7;
/// The feature register holding ID_AA64MMFR0_EL1.
pub const HV_FEATURE_REG_ID_AA64MMFR0_EL1: hv_feature_reg_t = 4;

/// The details of a vCPU exception: `ESR_EL2`, `FAR_EL2` and the faulting
/// intermediate physical address.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct hv_vcpu_exit_exception_t {
    /// The syndrome, in `ESR_EL2`.
    pub syndrome: u64,
    /// The faulting virtual address, in `FAR_EL2`. For a data abort, the
    /// address this reports is the guest's *virtual* address; the address the
    /// VMM must act on is [`Self::physical_address`].
    pub virtual_address: u64,
    /// The faulting intermediate physical (guest physical) address.
    pub physical_address: hv_ipa_t,
}

/// Why a vCPU returned from [`hv_vcpu_run`].
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct hv_vcpu_exit_t {
    /// One of `HV_EXIT_REASON_*`.
    pub reason: u32,
    /// Valid when `reason` is `HV_EXIT_REASON_EXCEPTION`.
    pub exception: hv_vcpu_exit_exception_t,
}

#[link(name = "Hypervisor", kind = "framework")]
unsafe extern "C" {
    // -- the VM ----------------------------------------------------------

    /// Creates a VM instance for the current process. Only one VM can exist
    /// per process.
    pub fn hv_vm_create(config: hv_vm_config_t) -> hv_return_t;
    /// Destroys the VM instance associated with the current process.
    pub fn hv_vm_destroy() -> hv_return_t;
    /// Maps a region of this process's address space into the VM's guest
    /// physical address space.
    pub fn hv_vm_map(
        address: *mut c_void,
        ipa: hv_ipa_t,
        size: usize,
        flags: hv_memory_flags_t,
    ) -> hv_return_t;
    /// Unmaps a region of the VM's guest physical address space.
    pub fn hv_vm_unmap(ipa: hv_ipa_t, size: usize) -> hv_return_t;
    /// The default intermediate physical address size (macOS 13 and later).
    pub fn hv_vm_config_get_default_ipa_size(ipa_bit_length: *mut u32) -> hv_return_t;

    // -- the vCPUs -------------------------------------------------------

    /// Creates a vCPU instance for the current thread.
    pub fn hv_vcpu_create(
        vcpu: *mut hv_vcpu_t,
        exit: *mut *mut hv_vcpu_exit_t,
        config: hv_vcpu_config_t,
    ) -> hv_return_t;
    /// Destroys the vCPU instance associated with the current thread.
    pub fn hv_vcpu_destroy(vcpu: hv_vcpu_t) -> hv_return_t;
    /// Gets a general-purpose register.
    pub fn hv_vcpu_get_reg(vcpu: hv_vcpu_t, reg: hv_reg_t, value: *mut u64) -> hv_return_t;
    /// Sets a general-purpose register.
    pub fn hv_vcpu_set_reg(vcpu: hv_vcpu_t, reg: hv_reg_t, value: u64) -> hv_return_t;
    /// Gets a system register.
    pub fn hv_vcpu_get_sys_reg(vcpu: hv_vcpu_t, reg: hv_sys_reg_t, value: *mut u64) -> hv_return_t;
    /// Sets a system register.
    pub fn hv_vcpu_set_sys_reg(vcpu: hv_vcpu_t, reg: hv_sys_reg_t, value: u64) -> hv_return_t;
    /// Sets whether an interrupt of `interrupt` type is pending for the vCPU.
    pub fn hv_vcpu_set_pending_interrupt(
        vcpu: hv_vcpu_t,
        interrupt: hv_interrupt_type_t,
        pending: bool,
    ) -> hv_return_t;
    /// Runs the vCPU until the next exit.
    pub fn hv_vcpu_run(vcpu: hv_vcpu_t) -> hv_return_t;
    /// Forces an immediate exit of a set of vCPUs. Callable from any thread.
    pub fn hv_vcpus_exit(vcpus: *const hv_vcpu_t, vcpu_count: u32) -> hv_return_t;
    /// Sets whether the virtual timer is masked for the vCPU.
    pub fn hv_vcpu_set_vtimer_mask(vcpu: hv_vcpu_t, masked: bool) -> hv_return_t;
    /// Sets the virtual timer's offset, i.e. where the guest's view of the
    /// virtual counter starts.
    pub fn hv_vcpu_set_vtimer_offset(vcpu: hv_vcpu_t, offset: u64) -> hv_return_t;

    // -- vCPU configuration (macOS 13 and later) -------------------------

    /// Creates a vCPU configuration object.
    pub fn hv_vcpu_config_create() -> hv_vcpu_config_t;
    /// Destroys a vCPU configuration object created by
    /// [`hv_vcpu_config_create`]. The object is owned by the caller.
    pub fn hv_vcpu_config_destroy(config: hv_vcpu_config_t) -> hv_return_t;
    /// Reads a feature register's host value.
    pub fn hv_vcpu_config_get_feature_reg(
        config: hv_vcpu_config_t,
        feature_register: hv_feature_reg_t,
        value: *mut u64,
    ) -> hv_return_t;
}

/// The error type of every call in this module.
#[derive(Debug, thiserror::Error)]
pub enum HvfError {
    /// Hypervisor.framework call `{call}` failed with {code:#010x}: {hint}
    #[error("Hypervisor.framework call {call} failed with {code:#010x}: {hint}")]
    Call {
        /// The name of the call that failed.
        call: &'static str,
        /// The status code it returned.
        code: hv_return_t,
        /// What that status usually means here.
        hint: &'static str,
    },
    /// Guest memory at {ipa:#x} cannot be given to Hypervisor.framework: {reason}
    #[error("guest memory at {ipa:#x} cannot be given to Hypervisor.framework: {reason}")]
    Mapping {
        /// The guest physical address of the region.
        ipa: u64,
        /// What is wrong with it.
        reason: &'static str,
    },
}

/// Explains a status code in the terms this backend cares about.
const fn hint(code: hv_return_t) -> &'static str {
    match code {
        HV_BUSY => {
            "Hypervisor.framework allows one VM per process, so a second VM cannot be created here"
        }
        HV_DENIED => {
            "the process lacks the com.apple.security.hypervisor entitlement, or the operation \
             is not permitted"
        }
        HV_BAD_ARGUMENT => "an argument is out of range (misaligned memory, or a bad register)",
        HV_ILLEGAL_GUEST_STATE => "the guest state is inconsistent",
        HV_NO_RESOURCES => "the host is out of a resource",
        HV_NO_DEVICE => "the vCPU or VM handle is invalid",
        HV_FAULT => "the operation faulted",
        HV_UNSUPPORTED => "this host does not support the operation",
        HV_ERROR => "the operation failed",
        _ => "unrecognized status",
    }
}

/// Turns a Hypervisor.framework status code into a [`Result`].
pub fn check(call: &'static str, ret: hv_return_t) -> Result<(), HvfError> {
    if ret == HV_SUCCESS {
        Ok(())
    } else {
        Err(HvfError::Call {
            call,
            code: ret,
            hint: hint(ret),
        })
    }
}

/// Runs a Hypervisor.framework call and turns a failure into an error that
/// names the call.
///
/// Several calls take a pointer to a value that has to be initialized before
/// the call (`hv_vcpu_create` writes the handle and the exit structure), so
/// this macro is the only place the `unsafe` of those calls is written.
macro_rules! hv_try {
    ($name:ident ( $($arg:expr),* $(,)? )) => {
        $crate::hvf::sys::check(stringify!($name), unsafe { $crate::hvf::sys::$name($($arg),*) })
    };
}

pub(crate) use hv_try;

/// A null `hv_vcpu_config_t`: what `hv_vcpu_create` takes to get the
/// framework's defaults (the ID registers are adjusted afterwards, through
/// the system-register API).
pub const NO_VCPU_CONFIG: hv_vcpu_config_t = ptr::null_mut();

/// A null `hv_vm_config_t`: the framework's default VM configuration.
pub const NO_VM_CONFIG: hv_vm_config_t = ptr::null_mut();

#[cfg(test)]
mod tests {
    use super::*;

    /// The system-register identifiers above are derived from the encoding
    /// rather than transcribed; these are the values in Apple's header, so the
    /// derivation is checked against the header on every run.
    #[test]
    fn system_register_identifiers_match_the_system_header() {
        assert_eq!(sysreg(3, 0, 0, 0, 0), 0xc000, "MIDR_EL1");
        assert_eq!(HV_SYS_REG_MPIDR_EL1, 0xc005);
        assert_eq!(HV_SYS_REG_ID_AA64PFR0_EL1, 0xc020);
        assert_eq!(HV_SYS_REG_ID_AA64MMFR0_EL1, 0xc038);
        assert_eq!(HV_SYS_REG_SCTLR_EL1, 0xc080);
        assert_eq!(HV_SYS_REG_SPSR_EL1, 0xc200);
        assert_eq!(HV_SYS_REG_ELR_EL1, 0xc201);
        assert_eq!(HV_SYS_REG_ESR_EL1, 0xc290);
        assert_eq!(HV_SYS_REG_FAR_EL1, 0xc300);
        assert_eq!(HV_SYS_REG_VBAR_EL1, 0xc600);
        assert_eq!(HV_SYS_REG_CNTV_CTL_EL0, 0xdf19);
        assert_eq!(HV_SYS_REG_CNTV_CVAL_EL0, 0xdf1a);

        // The feature-register identifiers the ID-register adjustment reads
        // through a vCPU configuration object.
        assert_eq!(HV_FEATURE_REG_ID_AA64MMFR0_EL1, 4, "ID_AA64MMFR0_EL1");
        assert_eq!(HV_FEATURE_REG_ID_AA64PFR0_EL1, 7, "ID_AA64PFR0_EL1");
    }

    #[test]
    fn the_exit_structure_is_laid_out_like_the_frameworks() {
        // `hv_vcpu_exit_t` is a 32-bit reason followed by three 64-bit
        // words; the vCPU threads read it through a raw pointer, so its
        // layout is part of the contract.
        assert_eq!(size_of::<hv_vcpu_exit_exception_t>(), 24);
        assert_eq!(size_of::<hv_vcpu_exit_t>(), 32);
        assert_eq!(core::mem::offset_of!(hv_vcpu_exit_t, reason), 0);
        assert_eq!(core::mem::offset_of!(hv_vcpu_exit_t, exception), 8);
    }
}
