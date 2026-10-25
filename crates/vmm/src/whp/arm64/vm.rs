// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//! The arm64 partition: `WHvCreatePartition` through `WHvSetupPartition` with the GICv3 property, guest
//! memory, the vCPUs' registers, runs, cancellation and interrupt requests. Each call goes through the
//! `WinHvPlatform.dll` entry points [`crate::whp::sys::api`] loads (the same loader as the x64 backend).

// The partition calls are `unsafe` FFI into WinHvPlatform.dll, each with its own `// SAFETY:` note.
#![allow(unsafe_code)]

use std::ffi::c_void;

use windows_sys::Win32::System::Hypervisor::{
    WHV_INTERRUPT_CONTROL, WHV_PARTITION_HANDLE, WHV_PARTITION_PROPERTY_CODE, WHV_REGISTER_NAME,
    WHV_REGISTER_VALUE, WHvMapGpaRangeFlagExecute, WHvMapGpaRangeFlagRead, WHvMapGpaRangeFlagWrite,
    WHvPartitionPropertyCodeProcessorCount,
};

use super::abi;
use crate::whp::sys::{Api, WhpError, api, call, hypervisor_present};

/// One partition of this process: the VM the arm64 backend builds.
pub struct Partition {
    api: &'static Api,
    handle: WHV_PARTITION_HANDLE,
}

// SAFETY: a partition handle is an opaque kernel handle; WHP serialises the calls made on it, and the
// handle stays valid until `Drop`. Every method takes `&self`.
unsafe impl Send for Partition {}
// SAFETY: as above; WHP calls on one partition are safe from several threads at once.
unsafe impl Sync for Partition {}

impl Partition {
    /// Creates and sets up a partition with `vcpus` processors and the GICv3 at distributor `gicd`
    /// (the hypervisor emulates the interrupt controller). Every vCPU's redistributor is set later, per
    /// vCPU, by [`Partition::set_registers`].
    pub fn create(vcpus: u32, gicd: u64) -> Result<Partition, WhpError> {
        let api = api()?;
        if !hypervisor_present(api)? {
            return Err(WhpError::NoHypervisor);
        }
        let mut handle: WHV_PARTITION_HANDLE = 0;
        // SAFETY: `handle` is a writable handle slot.
        unsafe { call!(api, WHvCreatePartition(&mut handle))? };
        // From here on `Drop` deletes the partition on every error path.
        let partition = Partition { api, handle };
        partition.set_property(WHvPartitionPropertyCodeProcessorCount, &vcpus.to_le_bytes())?;
        partition.set_property(
            abi::PROPERTY_ARM64_IC_PARAMETERS as WHV_PARTITION_PROPERTY_CODE,
            &abi::ic_parameters(gicd),
        )?;
        // SAFETY: the handle is a created, not yet set up partition.
        unsafe { call!(api, WHvSetupPartition(handle))? };
        Ok(partition)
    }

    fn set_property(
        &self,
        code: WHV_PARTITION_PROPERTY_CODE,
        bytes: &[u8],
    ) -> Result<(), WhpError> {
        // SAFETY: `bytes` is readable for its whole length, which is the size passed.
        unsafe {
            call!(
                self.api,
                WHvSetPartitionProperty(
                    self.handle,
                    code,
                    bytes.as_ptr().cast::<c_void>(),
                    bytes.len() as u32,
                )
            )
        }
    }

    /// Maps `size` bytes of this process's memory at `host` into the guest at `gpa`, readable, writable
    /// and executable.
    ///
    /// # Safety
    ///
    /// `host..host + size` must be committed memory of this process that stays mapped until the
    /// partition is dropped.
    pub unsafe fn map_memory(&self, host: *const u8, gpa: u64, size: u64) -> Result<(), WhpError> {
        // SAFETY: the caller guarantees the host range's lifetime; WHP checks alignment and the GPA range.
        unsafe {
            call!(
                self.api,
                WHvMapGpaRange(
                    self.handle,
                    host.cast::<c_void>(),
                    gpa,
                    size,
                    WHvMapGpaRangeFlagRead | WHvMapGpaRangeFlagWrite | WHvMapGpaRangeFlagExecute,
                )
            )
        }
    }

    /// Creates virtual processor `index`.
    pub fn create_vcpu(&self, index: u32) -> Result<(), WhpError> {
        // SAFETY: the partition is set up; the index is the one vCPU this call creates.
        unsafe { call!(self.api, WHvCreateVirtualProcessor(self.handle, index, 0)) }
    }

    /// Writes arm64 registers of vCPU `index`: each `(name, value)` as a 64-bit register.
    pub fn set_registers(&self, index: u32, registers: &[(u32, u64)]) -> Result<(), WhpError> {
        let names: Vec<WHV_REGISTER_NAME> = registers
            .iter()
            .map(|(name, _)| *name as WHV_REGISTER_NAME)
            .collect();
        let values: Vec<WHV_REGISTER_VALUE> = registers
            .iter()
            .map(|(_, value)| WHV_REGISTER_VALUE { Reg64: *value })
            .collect();
        // SAFETY: both arrays are `names.len()` long, the count passed.
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

    /// Reads one arm64 register of vCPU `index` as a 64-bit value.
    pub fn get_register(&self, index: u32, name: u32) -> Result<u64, WhpError> {
        let name = name as WHV_REGISTER_NAME;
        let mut value = WHV_REGISTER_VALUE { Reg64: 0 };
        // SAFETY: one name and one value slot, the count passed.
        unsafe {
            call!(
                self.api,
                WHvGetVirtualProcessorRegisters(self.handle, index, &name, 1, &mut value)
            )?
        };
        // SAFETY: the call wrote the 64-bit member of the union.
        Ok(unsafe { value.Reg64 })
    }

    /// Runs vCPU `index` until it exits, filling `exit` with the exit context. `exit` must be 16-byte
    /// aligned and at least [`abi::EXIT_CONTEXT_SIZE`] bytes.
    pub fn run(&self, index: u32, exit: &mut [u8]) -> Result<(), WhpError> {
        // SAFETY: `exit` is writable for its whole length, which is the size passed.
        unsafe {
            call!(
                self.api,
                WHvRunVirtualProcessor(
                    self.handle,
                    index,
                    exit.as_mut_ptr().cast::<c_void>(),
                    exit.len() as u32,
                )
            )
        }
    }

    /// Ends vCPU `index`'s current run (from another thread): its `run` returns a canceled exit.
    pub fn cancel(&self, index: u32) -> Result<(), WhpError> {
        // SAFETY: the partition is set up; cancelling a vCPU that is not running is harmless.
        unsafe {
            call!(
                self.api,
                WHvCancelRunVirtualProcessor(self.handle, index, 0)
            )
        }
    }

    /// Requests an interrupt: the 32-byte `WHV_INTERRUPT_CONTROL` the arm64 layout describes
    /// ([`abi::interrupt_control`]).
    pub fn request_interrupt(&self, control: &[u8; 32]) -> Result<(), WhpError> {
        // SAFETY: the buffer is the 32-byte arm64 structure WHP reads for `size` bytes.
        unsafe {
            call!(
                self.api,
                WHvRequestInterrupt(
                    self.handle,
                    control.as_ptr().cast::<WHV_INTERRUPT_CONTROL>(),
                    control.len() as u32,
                )
            )
        }
    }
}

impl Drop for Partition {
    fn drop(&mut self) {
        // SAFETY: the handle was created by `create` and is deleted once, here. Its failure cannot be
        // reported from `drop`.
        let _ = unsafe { (self.api.WHvDeletePartition)(self.handle) };
    }
}
