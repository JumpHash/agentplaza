// Copyright 2020 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/arch/aarch64/gic/regs.rs (trimmed: the
// serde derives and the ITS register state belong to the snapshot format, which
// this port does not have).

// KVM-device boundary (see the allowlist in `lib.rs`): `kvm-ioctls` declares
// `DeviceFd::get_device_attr` as an `unsafe fn` because the caller owns the
// attribute's out-pointer (here `&mut val`, a correctly sized register slot);
// there is no safe wrapper for it. (Note: the audit that opened this slice
// assumed this call was safe — it is not, in `kvm-ioctls` 0.25, so this block is
// irreducible and is documented, not removed.)
#![allow(unsafe_code)]

use std::fmt::Debug;
use std::iter::StepBy;
use std::ops::Range;

use kvm_bindings::kvm_device_attr;
use kvm_ioctls::DeviceFd;

use crate::arch::aarch64::gic::GicError;

#[derive(Debug)]
pub struct GicRegState<T> {
    pub(crate) chunks: Vec<T>,
}

/// Structure for serializing the state of the Vgic ICC regs
#[derive(Debug, Default)]
pub struct VgicSysRegsState {
    pub main_icc_regs: Vec<GicRegState<u64>>,
    pub ap_icc_regs: Vec<Option<GicRegState<u64>>>,
}

/// Structure holding the state of the GIC registers.
#[derive(Debug, Default)]
pub struct GicState {
    /// The state of the distributor registers.
    pub dist: Vec<GicRegState<u32>>,
    /// The state of the vcpu interfaces.
    pub gic_vcpu_states: Vec<GicVcpuState>,
}

/// Structure holding the state of the GIC registers for a specific vCPU.
#[derive(Debug, Default)]
pub struct GicVcpuState {
    pub rdist: Vec<GicRegState<u32>>,
    pub icc: VgicSysRegsState,
}

pub(crate) trait MmioReg {
    fn range(&self) -> Range<u64>;

    fn iter<T>(&self) -> StepBy<Range<u64>>
    where
        Self: Sized,
    {
        self.range().step_by(std::mem::size_of::<T>())
    }
}

pub(crate) trait VgicRegEngine {
    type Reg: MmioReg;
    type RegChunk: Clone + Default;

    fn group() -> u32;

    fn mpidr_mask() -> u64 {
        0
    }

    fn kvm_device_attr(offset: u64, val: &mut Self::RegChunk, mpidr: u64) -> kvm_device_attr {
        kvm_device_attr {
            group: Self::group(),
            attr: (mpidr & Self::mpidr_mask()) | offset,
            addr: val as *mut Self::RegChunk as u64,
            flags: 0,
        }
    }

    #[inline]
    fn get_reg_data(
        fd: &DeviceFd,
        reg: &Self::Reg,
        mpidr: u64,
    ) -> Result<GicRegState<Self::RegChunk>, GicError>
    where
        Self: Sized,
    {
        let mut data = Vec::with_capacity(reg.iter::<Self::RegChunk>().count());
        for offset in reg.iter::<Self::RegChunk>() {
            let mut val = Self::RegChunk::default();
            // SAFETY: `val` is a mutable memory location sized correctly for the attribute we're
            // requesting
            unsafe {
                fd.get_device_attr(&mut Self::kvm_device_attr(offset, &mut val, mpidr))
                    .map_err(|err| GicError::DeviceAttribute(err, false, Self::group()))?;
            }
            data.push(val);
        }

        Ok(GicRegState { chunks: data })
    }

    fn get_regs_data(
        fd: &DeviceFd,
        regs: Box<dyn Iterator<Item = &Self::Reg>>,
        mpidr: u64,
    ) -> Result<Vec<GicRegState<Self::RegChunk>>, GicError>
    where
        Self: Sized,
    {
        let mut data = Vec::new();
        for reg in regs {
            data.push(Self::get_reg_data(fd, reg, mpidr)?);
        }

        Ok(data)
    }

    #[inline]
    fn set_reg_data(
        fd: &DeviceFd,
        reg: &Self::Reg,
        data: &GicRegState<Self::RegChunk>,
        mpidr: u64,
    ) -> Result<(), GicError>
    where
        Self: Sized,
    {
        for (offset, val) in reg.iter::<Self::RegChunk>().zip(&data.chunks) {
            fd.set_device_attr(&Self::kvm_device_attr(offset, &mut val.clone(), mpidr))
                .map_err(|err| GicError::DeviceAttribute(err, true, Self::group()))?;
        }

        Ok(())
    }

    fn set_regs_data(
        fd: &DeviceFd,
        regs: Box<dyn Iterator<Item = &Self::Reg>>,
        data: &[GicRegState<Self::RegChunk>],
        mpidr: u64,
    ) -> Result<(), GicError>
    where
        Self: Sized,
    {
        for (reg, reg_data) in regs.zip(data) {
            Self::set_reg_data(fd, reg, reg_data, mpidr)?;
        }

        Ok(())
    }
}

/// Structure representing a simple register.
#[derive(PartialEq)]
pub(crate) struct SimpleReg {
    /// The offset from the component address. The register is memory mapped here.
    offset: u64,
    /// Size in bytes.
    size: u16,
}

impl SimpleReg {
    pub const fn new(offset: u64, size: u16) -> SimpleReg {
        SimpleReg { offset, size }
    }
}

impl MmioReg for SimpleReg {
    fn range(&self) -> Range<u64> {
        self.offset..self.offset + u64::from(self.size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A register's range covers exactly its declared size, and the chunk
    /// iterator walks it in strides of the chunk size: this arithmetic decides
    /// which offsets are read from (or written to) KVM.
    #[test]
    fn simple_reg_ranges_and_iteration() {
        let reg = SimpleReg::new(0x0400, 8);
        assert_eq!(reg.range(), 0x0400..0x0408);
        assert_eq!(reg.iter::<u32>().collect::<Vec<_>>(), vec![0x0400, 0x0404]);
        assert_eq!(reg.iter::<u64>().collect::<Vec<_>>(), vec![0x0400]);

        let reg = SimpleReg::new(0x0, 1);
        assert_eq!(reg.iter::<u8>().collect::<Vec<_>>(), vec![0x0]);
    }

    /// The engine encodes the register offset in the low 32 bits of the
    /// attribute and the vCPU affinity in the bits above, per the kernel's
    /// `KVM_DEV_ARM_VGIC_GRP_CPU_REGS` layout.
    #[test]
    fn kvm_device_attribute_encoding() {
        struct Engine;

        impl VgicRegEngine for Engine {
            type Reg = SimpleReg;
            type RegChunk = u64;

            fn group() -> u32 {
                0x2
            }

            fn mpidr_mask() -> u64 {
                0xff_0000_0000
            }
        }

        let mut val = 0_u64;
        let expected_addr = std::ptr::from_mut(&mut val) as u64;
        // The real mask keeps the affinity bits of the MPIDR (bits 32..40).
        let attr = Engine::kvm_device_attr(0x40, &mut val, 0x1_0000_0005);
        assert_eq!(attr.group, 0x2);
        assert_eq!(attr.attr, 0x1_0000_0040);
        assert_eq!(attr.addr, expected_addr);
        assert_eq!(attr.flags, 0);
    }
}
