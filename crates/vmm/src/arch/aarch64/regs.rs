// Copyright 2019 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Portions Copyright 2017 The Chromium OS Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the THIRD-PARTY file.
//
// Derived from Firecracker src/vmm/src/arch/aarch64/regs.rs (trimmed: the
// `Aarch64RegisterVec`/`RegSize`/`Aarch64RegisterData` machinery and its serde
// support belong to the snapshot and CPU-template paths (which this port does
// not have), as does the SVE vector-lengths pseudo-register; the register-ID
// construction — the core-register and system-register macros, the register
// table and `reg_size` — is kept, with its `/dev/kvm`-free tests).

use std::mem::offset_of;

use kvm_bindings::*;

#[allow(non_upper_case_globals)]
/// PSR (Processor State Register) bits.
/// Taken from arch/arm64/include/uapi/asm/ptrace.h.
const PSR_MODE_EL1h: u64 = 0x0000_0005;
const PSR_F_BIT: u64 = 0x0000_0040;
const PSR_I_BIT: u64 = 0x0000_0080;
const PSR_A_BIT: u64 = 0x0000_0100;
const PSR_D_BIT: u64 = 0x0000_0200;
/// Taken from arch/arm64/kvm/inject_fault.c.
pub const PSTATE_FAULT_BITS_64: u64 = PSR_MODE_EL1h | PSR_A_BIT | PSR_F_BIT | PSR_I_BIT | PSR_D_BIT;

/// Gets a core id.
macro_rules! arm64_core_reg_id {
    ($size: ident, $offset: expr) => {
        // The core registers of an arm64 machine are represented
        // in kernel by the `kvm_regs` structure. This structure is a
        // mix of 32, 64 and 128 bit fields:
        // struct kvm_regs {
        //     struct user_pt_regs      regs;
        //
        //     __u64                    sp_el1;
        //     __u64                    elr_el1;
        //
        //     __u64                    spsr[KVM_NR_SPSR];
        //
        //     struct user_fpsimd_state fp_regs;
        // };
        // struct user_pt_regs {
        //     __u64 regs[31];
        //     __u64 sp;
        //     __u64 pc;
        //     __u64 pstate;
        // };
        // The id of a core register can be obtained like this:
        // offset = id & ~(KVM_REG_ARCH_MASK | KVM_REG_SIZE_MASK | KVM_REG_ARM_CORE). Thus,
        // id = KVM_REG_ARM64 | KVM_REG_SIZE_U64/KVM_REG_SIZE_U32/KVM_REG_SIZE_U128 |
        // KVM_REG_ARM_CORE | offset
        KVM_REG_ARM64 as u64
            | KVM_REG_ARM_CORE as u64
            | $size
            | ($offset / std::mem::size_of::<u32>()) as u64
    };
}
pub(crate) use arm64_core_reg_id;

/// This macro computes the ID of a specific ARM64 system register similar to how
/// the kernel C macro does.
/// https://elixir.bootlin.com/linux/v4.20.17/source/arch/arm64/include/uapi/asm/kvm.h#L203
macro_rules! arm64_sys_reg {
    ($name: tt, $op0: tt, $op1: tt, $crn: tt, $crm: tt, $op2: tt) => {
        /// System register constant
        pub const $name: u64 = KVM_REG_ARM64 as u64
            | KVM_REG_SIZE_U64 as u64
            | KVM_REG_ARM64_SYSREG as u64
            | ((($op0 as u64) << KVM_REG_ARM64_SYSREG_OP0_SHIFT)
                & KVM_REG_ARM64_SYSREG_OP0_MASK as u64)
            | ((($op1 as u64) << KVM_REG_ARM64_SYSREG_OP1_SHIFT)
                & KVM_REG_ARM64_SYSREG_OP1_MASK as u64)
            | ((($crn as u64) << KVM_REG_ARM64_SYSREG_CRN_SHIFT)
                & KVM_REG_ARM64_SYSREG_CRN_MASK as u64)
            | ((($crm as u64) << KVM_REG_ARM64_SYSREG_CRM_SHIFT)
                & KVM_REG_ARM64_SYSREG_CRM_MASK as u64)
            | ((($op2 as u64) << KVM_REG_ARM64_SYSREG_OP2_SHIFT)
                & KVM_REG_ARM64_SYSREG_OP2_MASK as u64);
    };
}

// Constants imported from the Linux kernel:
// https://elixir.bootlin.com/linux/v4.20.17/source/arch/arm64/include/asm/sysreg.h#L135
arm64_sys_reg!(MPIDR_EL1, 3, 0, 0, 0, 5);
arm64_sys_reg!(MIDR_EL1, 3, 0, 0, 0, 0);

// ID registers that represent cpu capabilities.
// Needed for static cpu templates.
arm64_sys_reg!(ID_AA64PFR0_EL1, 3, 0, 0, 4, 0);
arm64_sys_reg!(ID_AA64ISAR0_EL1, 3, 0, 0, 6, 0);
arm64_sys_reg!(ID_AA64ISAR1_EL1, 3, 0, 0, 6, 1);
arm64_sys_reg!(ID_AA64MMFR2_EL1, 3, 0, 0, 7, 2);

// Cache Level ID Register
// https://developer.arm.com/documentation/ddi0595/2021-12/AArch64-Registers/CLIDR-EL1--Cache-Level-ID-Register
arm64_sys_reg!(CLIDR_EL1, 3, 1, 0, 0, 1);

// Counter-timer Virtual Timer CompareValue register.
// https://developer.arm.com/documentation/ddi0595/2021-12/AArch64-Registers/CNTV-CVAL-EL0--Counter-timer-Virtual-Timer-CompareValue-register
// https://elixir.bootlin.com/linux/v6.8/source/arch/arm64/include/asm/sysreg.h#L468
arm64_sys_reg!(SYS_CNTV_CVAL_EL0, 3, 3, 14, 3, 2);

// Counter-timer Physical Count Register
// https://developer.arm.com/documentation/ddi0601/2023-12/AArch64-Registers/CNTPCT-EL0--Counter-timer-Physical-Count-Register
// https://elixir.bootlin.com/linux/v6.8/source/arch/arm64/include/asm/sysreg.h#L459
arm64_sys_reg!(SYS_CNTPCT_EL0, 3, 3, 14, 0, 1);

// Physical Timer EL0 count Register
// The id of this register is same as SYS_CNTPCT_EL0, but KVM defines it
// separately, so we do as well.
// https://elixir.bootlin.com/linux/v6.12.6/source/arch/arm64/include/uapi/asm/kvm.h#L259
arm64_sys_reg!(KVM_REG_ARM_PTIMER_CNT, 3, 3, 14, 0, 1);

// Translation Table Base Register
// https://developer.arm.com/documentation/ddi0595/2021-03/AArch64-Registers/TTBR1-EL1--Translation-Table-Base-Register-1--EL1-
arm64_sys_reg!(TTBR1_EL1, 3, 0, 2, 0, 1);
// Translation Control Register
// https://developer.arm.com/documentation/ddi0601/2024-09/AArch64-Registers/TCR-EL1--Translation-Control-Register--EL1-
arm64_sys_reg!(TCR_EL1, 3, 0, 2, 0, 2);
// AArch64 Memory Model Feature Register
// https://developer.arm.com/documentation/100798/0400/register-descriptions/aarch64-system-registers/id-aa64mmfr0-el1--aarch64-memory-model-feature-register-0--el1
arm64_sys_reg!(ID_AA64MMFR0_EL1, 3, 0, 0, 7, 0);

/// Program Counter
/// The offset value (0x100 = 32 * 8) is calculated as follows:
/// - `kvm_regs` includes `regs` field of type `user_pt_regs` at the beginning (i.e., at offset 0).
/// - `pc` follows `regs[31]` and `sp` within `user_pt_regs` and they are 8 bytes each (i.e. the
///   offset is (31 + 1) * 8 = 256).
///
/// https://github.com/torvalds/linux/blob/master/Documentation/virt/kvm/api.rst#L2578
/// > 0x6030 0000 0010 0040 PC          64  regs.pc
pub const PC: u64 = {
    let kreg_off = offset_of!(kvm_regs, regs);
    let pc_off = offset_of!(user_pt_regs, pc);
    arm64_core_reg_id!(KVM_REG_SIZE_U64, kreg_off + pc_off)
};

/// Returns register size in bytes
pub fn reg_size(reg_id: u64) -> usize {
    2_usize.pow(((reg_id & KVM_REG_SIZE_MASK) >> KVM_REG_SIZE_SHIFT) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reg_size() {
        assert_eq!(reg_size(KVM_REG_SIZE_U32), 4);
        // ID_AA64PFR0_EL1 is 64 bit register
        assert_eq!(reg_size(ID_AA64PFR0_EL1), 8);
    }

    /// The core-register ID encodes the byte offset divided by four, so the
    /// program counter must land on the value documented by KVM's ABI.
    #[test]
    fn test_core_reg_ids() {
        assert_eq!(PC, 0x6030_0000_0010_0040);

        // `pstate` is the field after `pc` in `user_pt_regs`; `regs[0]` is
        // the first field of the structure. Both must keep the 8-byte stride
        // of the ABI.
        let pstate = arm64_core_reg_id!(KVM_REG_SIZE_U64, offset_of!(user_pt_regs, pstate));
        let reg0 = arm64_core_reg_id!(KVM_REG_SIZE_U64, offset_of!(user_pt_regs, regs));
        assert_eq!(pstate - PC, 8 / 4);
        assert_eq!(
            reg0,
            KVM_REG_ARM64 as u64 | KVM_REG_ARM_CORE as u64 | KVM_REG_SIZE_U64
        );
    }

    /// System-register IDs are `KVM_REG_ARM64 | KVM_REG_SIZE_U64 |
    /// KVM_REG_ARM64_SYSREG` with the `op0:op1:CRn:CRm:op2` encoding in the
    /// low bits, so two different system registers never collide.
    #[test]
    fn test_sys_reg_ids() {
        let encoding = |id: u64| {
            id & !(u64::from(KVM_REG_ARM64)
                | u64::from(KVM_REG_SIZE_U64)
                | u64::from(KVM_REG_ARM64_SYSREG))
        };
        assert_eq!(reg_size(MPIDR_EL1), 8);
        assert_ne!(encoding(MPIDR_EL1), encoding(MIDR_EL1));
        assert_ne!(encoding(CLIDR_EL1), encoding(SYS_CNTV_CVAL_EL0));
        // MPIDR_EL1 is op0=3, op1=0, CRn=0, CRm=0, op2=5, i.e.
        // 5 << 0 | 0 << 3 | 0 << 7 | 0 << 11 | 3 << 14.
        assert_eq!(encoding(MPIDR_EL1), (3 << 14) | 5);
    }
}
