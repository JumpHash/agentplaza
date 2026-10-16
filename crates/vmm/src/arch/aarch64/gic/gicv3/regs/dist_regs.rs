// Copyright 2020 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/arch/aarch64/gic/gicv3/regs/dist_regs.rs
// (trimmed: the register table and its `/dev/kvm`-free constructor tests are
// upstream's; the `/dev/kvm`-dependent access test is dropped).

use std::ops::Range;

use kvm_bindings::KVM_DEV_ARM_VGIC_GRP_DIST_REGS;
use kvm_ioctls::DeviceFd;

use crate::arch::aarch64::gic::GicError;
use crate::arch::aarch64::gic::regs::{GicRegState, MmioReg, SimpleReg, VgicRegEngine};
use crate::arch::aarch64::layout::{GSI_LEGACY_NUM, SPI_START};

// Distributor registers as detailed at page 456 from
// https://static.docs.arm.com/ihi0069/c/IHI0069C_gic_architecture_specification.pdf.
// Address offsets are relative to the Distributor base address defined
// by the system memory map.
const GICD_CTLR: DistReg = DistReg::simple(0x0, 4);
const GICD_STATUSR: DistReg = DistReg::simple(0x0010, 4);
const GICD_IGROUPR: DistReg = DistReg::shared_irq(0x0080, 1);
const GICD_ISENABLER: DistReg = DistReg::shared_irq(0x0100, 1);
const GICD_ICENABLER: DistReg = DistReg::shared_irq(0x0180, 1);
const GICD_ISPENDR: DistReg = DistReg::shared_irq(0x0200, 1);
const GICD_ICPENDR: DistReg = DistReg::shared_irq(0x0280, 1);
const GICD_ISACTIVER: DistReg = DistReg::shared_irq(0x0300, 1);
const GICD_ICACTIVER: DistReg = DistReg::shared_irq(0x0380, 1);
const GICD_IPRIORITYR: DistReg = DistReg::shared_irq(0x0400, 8);
const GICD_ICFGR: DistReg = DistReg::shared_irq(0x0C00, 2);
const GICD_IROUTER: DistReg = DistReg::shared_irq(0x6000, 64);

// List with relevant distributor registers that we will be restoring.
// Order is taken from qemu.
// Criteria for the present list of registers: only R/W registers, implementation specific registers
// are not saved. GICD_CPENDSGIR and GICD_SPENDSGIR are not saved since these registers are not used
// when affinity routing is enabled. Affinity routing GICv3 is enabled by default unless Firecracker
// clears the ICD_CTLR.ARE bit which it does not do.
static VGIC_DIST_REGS: &[DistReg] = &[
    GICD_CTLR,
    GICD_STATUSR,
    GICD_ICENABLER,
    GICD_ISENABLER,
    GICD_IGROUPR,
    GICD_IROUTER,
    GICD_ICFGR,
    GICD_ICPENDR,
    GICD_ISPENDR,
    GICD_ICACTIVER,
    GICD_ISACTIVER,
    GICD_IPRIORITYR,
];

/// Some registers have variable lengths since they dedicate a specific number of bits to
/// each interrupt. So, their length depends on the number of interrupts.
/// (i.e the ones that are represented as GICD_REG<n>) in the documentation mentioned above.
pub struct SharedIrqReg {
    /// The offset from the component address. The register is memory mapped here.
    offset: u64,
    /// Number of bits per interrupt.
    bits_per_irq: u8,
}

impl MmioReg for SharedIrqReg {
    fn range(&self) -> Range<u64> {
        // The ARM® TrustZone® implements a protection logic which contains a
        // read-as-zero/write-ignore (RAZ/WI) policy.
        // The first part of a shared-irq register, the one corresponding to the
        // SGI and PPI IRQs (0-32) is RAZ/WI, so we skip it.
        let start = self.offset + u64::from(SPI_START) * u64::from(self.bits_per_irq) / 8;

        let size_in_bits = u64::from(self.bits_per_irq) * u64::from(GSI_LEGACY_NUM);
        let mut size_in_bytes = size_in_bits / 8;
        if size_in_bits % 8 > 0 {
            size_in_bytes += 1;
        }

        start..start + size_in_bytes
    }
}

enum DistReg {
    Simple(SimpleReg),
    SharedIrq(SharedIrqReg),
}

impl DistReg {
    const fn simple(offset: u64, size: u16) -> DistReg {
        DistReg::Simple(SimpleReg::new(offset, size))
    }

    const fn shared_irq(offset: u64, bits_per_irq: u8) -> DistReg {
        DistReg::SharedIrq(SharedIrqReg {
            offset,
            bits_per_irq,
        })
    }
}

impl MmioReg for DistReg {
    fn range(&self) -> Range<u64> {
        match self {
            DistReg::Simple(reg) => reg.range(),
            DistReg::SharedIrq(reg) => reg.range(),
        }
    }
}

struct DistRegEngine {}

impl VgicRegEngine for DistRegEngine {
    type Reg = DistReg;
    type RegChunk = u32;

    fn group() -> u32 {
        KVM_DEV_ARM_VGIC_GRP_DIST_REGS
    }

    fn mpidr_mask() -> u64 {
        0
    }
}

pub(crate) fn get_dist_regs(fd: &DeviceFd) -> Result<Vec<GicRegState<u32>>, GicError> {
    DistRegEngine::get_regs_data(fd, Box::new(VGIC_DIST_REGS.iter()), 0)
}

pub(crate) fn set_dist_regs(fd: &DeviceFd, state: &[GicRegState<u32>]) -> Result<(), GicError> {
    DistRegEngine::set_regs_data(fd, Box::new(VGIC_DIST_REGS.iter()), state, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GICD_IROUTER is 64 bits per interrupt, so its SPI sub-range is the
    /// largest of the distributor's shared-irq registers.
    #[test]
    fn shared_irq_ranges_cover_only_the_spis() {
        let reg = DistReg::shared_irq(0x6000, 64);
        assert_eq!(reg.range(), 0x6100..0x6400);
        let reg = DistReg::shared_irq(0x0100, 1);
        assert_eq!(reg.range(), 0x0104..0x0110);
    }
}
