// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/arch/x86_64/interrupts.rs (trimmed:
// the `zerocopy` and `utils::byte_order` little-endian helpers are inlined).

use kvm_bindings::kvm_lapic_state;
use kvm_ioctls::VcpuFd;
use zerocopy::IntoBytes;

/// Errors thrown while configuring the LAPIC.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum InterruptError {
    /// Failure in getting the LAPIC configuration: {0}
    #[error("Failure in getting the LAPIC configuration: {0}")]
    GetLapic(kvm_ioctls::Error),
    /// Failure in setting the LAPIC configuration: {0}
    #[error("Failure in setting the LAPIC configuration: {0}")]
    SetLapic(kvm_ioctls::Error),
}

// Defines poached from apicdef.h kernel header.
const APIC_LVT0: usize = 0x350;
const APIC_LVT1: usize = 0x360;
const APIC_MODE_NMI: u32 = 0x4;
const APIC_MODE_EXTINT: u32 = 0x7;

/// Reads a little-endian u32 from a 4-byte slice (local equivalent of the
/// upstream `utils::byte_order::read_le_u32`).
fn read_le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().expect("read_le_u32 expects 4 bytes"))
}

/// Writes a little-endian u32 into a 4-byte slice (local equivalent of the
/// upstream `utils::byte_order::write_le_u32`).
fn write_le_u32(bytes: &mut [u8], value: u32) {
    bytes.copy_from_slice(&value.to_le_bytes());
}

fn get_klapic_reg(klapic: &kvm_lapic_state, reg_offset: usize) -> u32 {
    let range = reg_offset..reg_offset + 4;
    let reg = klapic.regs.get(range).expect("get_klapic_reg range");
    // `regs` is a plain `[c_char; 1024]`, so `IntoBytes` (zerocopy) views the
    // same POD memory as bytes without an unchecked cast.
    read_le_u32(reg.as_bytes())
}

fn set_klapic_reg(klapic: &mut kvm_lapic_state, reg_offset: usize, value: u32) {
    let range = reg_offset..reg_offset + 4;
    let reg = klapic.regs.get_mut(range).expect("set_klapic_reg range");
    // As in `get_klapic_reg`; the slice covers exactly the 4 bytes of the
    // register inside the array, which is uniquely borrowed here.
    write_le_u32(reg.as_mut_bytes(), value);
}

fn set_apic_delivery_mode(reg: u32, mode: u32) -> u32 {
    ((reg) & !0x700) | ((mode) << 8)
}

/// Configures LAPICs.  LAPIC0 is set for external interrupts, LAPIC1 is set for NMI.
///
/// # Arguments
/// * `vcpu` - The VCPU object to configure.
pub fn set_lint(vcpu: &VcpuFd) -> Result<(), InterruptError> {
    let mut klapic = vcpu.get_lapic().map_err(InterruptError::GetLapic)?;

    let lvt_lint0 = get_klapic_reg(&klapic, APIC_LVT0);
    set_klapic_reg(
        &mut klapic,
        APIC_LVT0,
        set_apic_delivery_mode(lvt_lint0, APIC_MODE_EXTINT),
    );
    let lvt_lint1 = get_klapic_reg(&klapic, APIC_LVT1);
    set_klapic_reg(
        &mut klapic,
        APIC_LVT1,
        set_apic_delivery_mode(lvt_lint1, APIC_MODE_NMI),
    );

    vcpu.set_lapic(&klapic).map_err(InterruptError::SetLapic)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KVM_APIC_REG_SIZE: usize = 0x400;

    #[test]
    fn test_set_and_get_klapic_reg() {
        let reg_offset = 0x340;
        for (case, value) in [
            ("a small value", 3u32),
            (
                "a value past i32::MAX",
                u32::try_from(i32::MAX).unwrap() + 1u32,
            ),
        ] {
            let mut klapic = kvm_lapic_state::default();
            set_klapic_reg(&mut klapic, reg_offset, value);
            let got = get_klapic_reg(&klapic, reg_offset);
            assert_eq!(got, value, "{case}");
        }
    }

    #[test]
    #[should_panic]
    fn test_set_and_get_klapic_out_of_bounds() {
        let reg_offset = KVM_APIC_REG_SIZE + 10;
        let mut klapic = kvm_lapic_state::default();
        set_klapic_reg(&mut klapic, reg_offset, 3);
    }

    #[test]
    fn test_apic_delivery_mode() {
        // A spread of values with bits set across the delivery-mode field
        // (upstream seeds this from `vmm_sys_util::rand`, which this port does
        // not depend on).
        let v: Vec<u32> = (0..20u32).map(|x| x.wrapping_mul(2_654_435_761)).collect();

        let after: Vec<u32> = v.iter().map(|x| (*x & !0x700) | ((2) << 8)).collect();
        let set: Vec<u32> = v.iter().map(|x| set_apic_delivery_mode(*x, 2)).collect();
        assert_eq!(set, after);
    }
}
