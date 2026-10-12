// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: the WHP counterpart of `vstate::vm` (KVM) and
// `hvf::vm` (Hypervisor.framework).

//! One WHP partition: creation and setup, guest memory mappings, vCPU
//! creation, register access and interrupt requests.
//!
//! WHP calls on one partition may be made from any thread, so the partition
//! is shared by reference between the vCPU threads, the interrupt relay and
//! the stop control.

// FFI boundary (see the allowlist in `lib.rs`): the WHP calls themselves.
#![allow(unsafe_code)]

use std::ffi::c_void;

use windows_sys::Win32::System::Hypervisor::*;

use super::ioapic::{DeliveryMode, Interrupt};
use super::sys::{Api, WhpError, api, call, hypervisor_present};

/// A WHP partition, deleted on drop.
pub struct Partition {
    api: &'static Api,
    handle: WHV_PARTITION_HANDLE,
}

// SAFETY: a partition handle is an opaque kernel handle; WHP serialises the
// calls made on it, and the handle stays valid until `Drop`.
unsafe impl Send for Partition {}
// SAFETY: as above - every method takes `&self` and WHP calls on one
// partition are safe from several threads at once.
unsafe impl Sync for Partition {}

impl Partition {
    /// Creates and sets up a partition with `vcpus` processors and WHP's
    /// xAPIC emulation.
    pub fn create(vcpus: u32) -> Result<Partition, WhpError> {
        let api = api()?;
        if !hypervisor_present(api)? {
            return Err(WhpError::NoHypervisor);
        }
        let mut handle: WHV_PARTITION_HANDLE = 0;
        // SAFETY: `handle` is a writable handle slot.
        unsafe { call!(api, WHvCreatePartition(&mut handle))? };
        // From here on, `Drop` deletes the partition on every error path.
        let partition = Partition { api, handle };

        let mut property = WHV_PARTITION_PROPERTY::default();
        property.ProcessorCount = vcpus;
        partition.set_property(
            WHvPartitionPropertyCodeProcessorCount,
            &property,
            std::mem::size_of::<u32>(),
        )?;

        // The local APICs are emulated by the hypervisor (xAPIC mode: the guest
        // finds them at 0xFEE0_0000, as the MADT and the MPTable say); the I/O
        // APIC is this process's (`super::ioapic`). Level-triggered interrupts
        // then end with an `X64ApicEoi` exit carrying the vector.
        let mut property = WHV_PARTITION_PROPERTY::default();
        property.LocalApicEmulationMode = WHvX64LocalApicEmulationModeXApic;
        partition.set_property(
            WHvPartitionPropertyCodeLocalApicEmulationMode,
            &property,
            std::mem::size_of::<WHV_X64_LOCAL_APIC_EMULATION_MODE>(),
        )?;

        // CPUID leaf 1 exits to this process (see `vcpu::cpuid`): the
        // hypervisor's answer is passed through with x2APIC masked, since only
        // the xAPIC is emulated.
        let mut property = WHV_PARTITION_PROPERTY::default();
        property.ExtendedVmExits.AsUINT64 = 1; // X64CpuidExit
        partition.set_property(
            WHvPartitionPropertyCodeExtendedVmExits,
            &property,
            std::mem::size_of::<u64>(),
        )?;
        let mut property = WHV_PARTITION_PROPERTY::default();
        property.CpuidExitList = [1];
        partition.set_property(
            WHvPartitionPropertyCodeCpuidExitList,
            &property,
            std::mem::size_of::<u32>(),
        )?;

        // SAFETY: the handle is a created, not yet set up partition.
        unsafe { call!(api, WHvSetupPartition(handle))? };
        Ok(partition)
    }

    fn set_property(
        &self,
        code: WHV_PARTITION_PROPERTY_CODE,
        property: &WHV_PARTITION_PROPERTY,
        size: usize,
    ) -> Result<(), WhpError> {
        // SAFETY: `property` is a valid `WHV_PARTITION_PROPERTY` of which the
        // first `size` bytes hold the property `code` names.
        unsafe {
            call!(
                self.api,
                WHvSetPartitionProperty(
                    self.handle,
                    code,
                    (property as *const WHV_PARTITION_PROPERTY).cast::<c_void>(),
                    size as u32,
                )
            )
        }
    }

    /// Maps `size` bytes of this process's memory at `host` into the guest at
    /// `gpa`, readable, writable and executable.
    ///
    /// # Safety
    ///
    /// `host..host + size` must be committed memory of this process that stays
    /// mapped until the partition is dropped (the guest reads and writes it
    /// behind the compiler's back, as it does through any other mapping).
    pub unsafe fn map_memory(&self, host: *mut u8, gpa: u64, size: u64) -> Result<(), WhpError> {
        // SAFETY: the caller guarantees the host range's lifetime; WHP checks
        // the alignment and the GPA range itself.
        unsafe {
            call!(
                self.api,
                WHvMapGpaRange(
                    self.handle,
                    host.cast::<c_void>().cast_const(),
                    gpa,
                    size,
                    WHvMapGpaRangeFlagRead | WHvMapGpaRangeFlagWrite | WHvMapGpaRangeFlagExecute,
                )
            )
        }
    }

    /// Creates virtual processor `index`.
    pub fn create_vcpu(&self, index: u32) -> Result<(), WhpError> {
        // SAFETY: plain call on a set-up partition.
        unsafe { call!(self.api, WHvCreateVirtualProcessor(self.handle, index, 0)) }
    }

    /// Runs virtual processor `index` until its next exit.
    pub fn run_vcpu(&self, index: u32, exit: &mut WHV_RUN_VP_EXIT_CONTEXT) -> Result<(), WhpError> {
        // SAFETY: `exit` is a writable exit context of the size passed.
        unsafe {
            call!(
                self.api,
                WHvRunVirtualProcessor(
                    self.handle,
                    index,
                    (exit as *mut WHV_RUN_VP_EXIT_CONTEXT).cast::<c_void>(),
                    std::mem::size_of::<WHV_RUN_VP_EXIT_CONTEXT>() as u32,
                )
            )
        }
    }

    /// Makes a running `run_vcpu(index)` return with a `Canceled` exit.
    pub fn cancel_vcpu(&self, index: u32) -> Result<(), WhpError> {
        // SAFETY: plain call; cancelling a processor that is not running makes
        // its next run return at once.
        unsafe { call!(self.api, WHvCancelRunVirtualProcessor(self.handle, index, 0)) }
    }

    /// Reads registers of processor `index`.
    pub fn get_registers(
        &self,
        index: u32,
        names: &[WHV_REGISTER_NAME],
        values: &mut [WHV_REGISTER_VALUE],
    ) -> Result<(), WhpError> {
        assert_eq!(names.len(), values.len());
        // SAFETY: `names` and `values` are arrays of the same, passed length.
        unsafe {
            call!(
                self.api,
                WHvGetVirtualProcessorRegisters(
                    self.handle,
                    index,
                    names.as_ptr(),
                    names.len() as u32,
                    values.as_mut_ptr(),
                )
            )
        }
    }

    /// Writes registers of processor `index`.
    pub fn set_registers(
        &self,
        index: u32,
        names: &[WHV_REGISTER_NAME],
        values: &[WHV_REGISTER_VALUE],
    ) -> Result<(), WhpError> {
        assert_eq!(names.len(), values.len());
        // SAFETY: `names` and `values` are arrays of the same, passed length.
        unsafe {
            call!(
                self.api,
                WHvSetVirtualProcessorRegisters(
                    self.handle,
                    index,
                    names.as_ptr(),
                    names.len() as u32,
                    values.as_ptr(),
                )
            )
        }
    }

    /// Translates a guest virtual address of processor `index` (for the
    /// instruction emulator).
    pub fn translate_gva(
        &self,
        index: u32,
        gva: u64,
        flags: WHV_TRANSLATE_GVA_FLAGS,
    ) -> Result<(WHV_TRANSLATE_GVA_RESULT_CODE, u64), WhpError> {
        let mut result = WHV_TRANSLATE_GVA_RESULT::default();
        let mut gpa = 0u64;
        // SAFETY: `result` and `gpa` are writable out-parameters.
        unsafe {
            call!(
                self.api,
                WHvTranslateGva(self.handle, index, gva, flags, &mut result, &mut gpa)
            )?;
        }
        Ok((result.ResultCode, gpa))
    }

    /// Asks the hypervisor's local APIC emulation to deliver `interrupt`.
    pub fn request_interrupt(&self, interrupt: Interrupt) -> Result<(), WhpError> {
        let control = interrupt_control(interrupt);
        // SAFETY: `control` is a valid `WHV_INTERRUPT_CONTROL` of the size passed.
        unsafe {
            call!(
                self.api,
                WHvRequestInterrupt(
                    self.handle,
                    &control,
                    std::mem::size_of::<WHV_INTERRUPT_CONTROL>() as u32,
                )
            )
        }
    }

    /// The API table, for the instruction emulator.
    pub fn api(&self) -> &'static Api {
        self.api
    }
}

impl Drop for Partition {
    fn drop(&mut self) {
        // SAFETY: the handle is a created partition that nothing uses any more
        // (every vCPU thread holds the partition through the VM, which is only
        // dropped after they finish).
        if let Err(err) = unsafe { call!(self.api, WHvDeletePartition(self.handle)) } {
            eprintln!("whp: deleting the partition failed: {err}");
        }
    }
}

/// The `WHV_INTERRUPT_CONTROL` for an I/O APIC delivery: type in bits 0..7,
/// destination mode in bits 8..11, trigger mode in bits 12..15.
fn interrupt_control(interrupt: Interrupt) -> WHV_INTERRUPT_CONTROL {
    let kind = match interrupt.delivery {
        DeliveryMode::Fixed | DeliveryMode::ExtInt | DeliveryMode::Smi => {
            WHvX64InterruptTypeFixed
        }
        DeliveryMode::LowestPriority => WHvX64InterruptTypeLowestPriority,
        DeliveryMode::Nmi => WHvX64InterruptTypeNmi,
        DeliveryMode::Init => WHvX64InterruptTypeInit,
    } as u64;
    let mode = if interrupt.logical {
        WHvX64InterruptDestinationModeLogical
    } else {
        WHvX64InterruptDestinationModePhysical
    } as u64;
    let trigger = if interrupt.level_triggered {
        WHvX64InterruptTriggerModeLevel
    } else {
        WHvX64InterruptTriggerModeEdge
    } as u64;
    WHV_INTERRUPT_CONTROL {
        _bitfield: (kind & 0xff) | ((mode & 0xf) << 8) | ((trigger & 0xf) << 12),
        Destination: u32::from(interrupt.destination),
        Vector: u32::from(interrupt.vector),
    }
}
