// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port. Firecracker has no Hypervisor.framework backend, so
// there is nothing upstream to copy. The exception classes, the instruction
// syndrome layouts and the rule for which exits advance the guest's program
// counter come from the Arm Architecture Reference Manual (ESR_EL2) and were
// checked against what a VMM must do for each class - QEMU's
// `target/arm/hvf/hvf.c` `hvf_handle_exception()` is the reference
// implementation of the same mapping, and it is GPL-2.0, so it was read for
// the exit protocol (which exits advance the PC, which fault address is the
// IPA) but no code was copied from it.

//! Decoding of a vCPU exit: what the guest did, and what the backend must do
//! about it.
//!
//! This module is free of FFI and of any `crate::arch` dependency so it builds
//! and is unit-tested on the host.

/// A data abort taken from a lower exception level: the guest's own load or
/// store faulted into the hypervisor, which is how a device access reaches the
/// VMM (the device's address has no guest memory behind it).
pub const EC_DATAABORT_LOWER: u8 = 0x24;
/// An instruction abort from a lower exception level: the guest tried to fetch
/// from an address it has no memory or permission for.
pub const EC_INSNABORT_LOWER: u8 = 0x20;
/// A trapped `MRS`/`MSR`: the guest touched a system register Hypervisor.framework
/// leaves to the VMM (the `ICC_*` registers of a userspace interrupt
/// controller, among others).
pub const EC_SYSREG_TRAP: u8 = 0x18;
/// An `SMC` instruction.
pub const EC_AA64_SMC: u8 = 0x17;
/// An `HVC` instruction: the conduit a PSCI call arrives on.
pub const EC_AA64_HVC: u8 = 0x16;
/// A `WFI`/`WFE` trap.
pub const EC_WFX_TRAP: u8 = 0x01;

/// `ESR_EL2.ISS.WFx` in a `WFI`/`WFE` trap: set for `WFE`.
pub const WFX_ISS_IS_WFE: u64 = 1 << 0;

/// The hypervisor encoding of a system register:
/// `(op0 << 14) | (op1 << 11) | (crn << 7) | (crm << 3) | op2`.
///
/// This is the kernel's `sys_reg()` packing, and it is also how
/// Hypervisor.framework's `hv_sys_reg_t` values (`HV_SYS_REG_MIDR_EL1` and
/// friends) are built. [`SysReg::decode`] derives it from an `ESR_EL2`
/// instruction syndrome, where the same five fields live at other shifts.
pub const fn sysreg(op0: u16, op1: u16, crn: u16, crm: u16, op2: u16) -> u16 {
    (op0 << 14) | (op1 << 11) | (crn << 7) | (crm << 3) | op2
}

/// The exception class of an `ESR_EL2` value.
pub const fn exception_class(syndrome: u64) -> u8 {
    ((syndrome >> 26) & 0x3f) as u8
}

/// A trapped `MRS`/`MSR`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SysReg {
    /// `true` for `MRS` (a read of the register), `false` for `MSR`.
    pub read: bool,
    /// The general-purpose register the value is transferred to or from.
    pub rt: u8,
    /// The trapped register, in the [`sysreg`] encoding.
    pub reg: u16,
}

impl SysReg {
    /// Decodes the instruction syndrome of an [`EC_SYSREG_TRAP`].
    ///
    /// `ESR_EL2` packs the fields of the register as `op0` in bits 21:20,
    /// `op2` in 19:17, `op1` in 16:14, `crn` in 13:10 and `crm` in 4:1, with
    /// the access direction in bit 0 and the transfer register in bits 9:5.
    pub const fn decode(iss: u64) -> Self {
        let op0 = ((iss >> 20) & 0x3) as u16;
        let op1 = ((iss >> 14) & 0x7) as u16;
        let crn = ((iss >> 10) & 0xf) as u16;
        let crm = ((iss >> 1) & 0xf) as u16;
        let op2 = ((iss >> 17) & 0x7) as u16;
        Self {
            read: iss & 1 == 1,
            rt: ((iss >> 5) & 0x1f) as u8,
            reg: sysreg(op0, op1, crn, crm, op2),
        }
    }
}

/// A load or store that faulted into the hypervisor: an access to a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MmioAccess {
    /// `true` for a load, `false` for a store.
    pub read: bool,
    /// The general-purpose register holding (or receiving) the value.
    pub rt: u8,
    /// The access width in bytes: 1, 2, 4 or 8.
    pub len: u8,
    /// `SSE`: the loaded value is sign-extended to the register width.
    pub sign_extend: bool,
    /// `ISV`: the syndrome carries a valid instruction decode. Every plain
    /// `LDR`/`STR` to a device sets it; SIMD/SVE accesses do not, and the
    /// backend has to fault those back into the guest instead of emulating
    /// them.
    pub valid: bool,
    /// `CM`: a cache maintenance instruction that happened to fault, which the
    /// architecture makes the handler skip.
    pub cache_maintenance: bool,
}

impl MmioAccess {
    /// Decodes the instruction syndrome of a data abort.
    ///
    /// `ESR_EL2`'s data-abort syndrome packs `ISV` in bit 24, `SAS` (the
    /// access size, as a log2) in bits 23:22, `SSE` in bit 21, `SRT` (the
    /// transfer register) in bits 20:16, `CM` in bit 8, `S1PTW` in bit 7 and
    /// `WnR` in bit 6.
    pub const fn decode(iss: u64) -> Self {
        let sas = (iss >> 22) & 0x3;
        Self {
            read: (iss >> 6) & 1 == 0,
            rt: ((iss >> 16) & 0x1f) as u8,
            len: (1 << sas) as u8,
            sign_extend: (iss >> 21) & 1 == 1,
            valid: (iss >> 24) & 1 == 1,
            cache_maintenance: (iss >> 8) & 1 == 1,
        }
    }
}

/// What a `WFI`/`WFE` trap asks of the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wait {
    /// `true` for `WFE`, `false` for `WFI`.
    pub event: bool,
}

/// A decoded vCPU exception.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exception {
    /// The guest executed `WFI`/`WFE` and the backend has to idle the vCPU
    /// until an interrupt, a timer expiry or a stop request arrives.
    Wait(Wait),
    /// `HVC #imm16`: the PSCI conduit. The exception return address is
    /// *after* the instruction, so the handler must not advance the PC.
    Hvc {
        /// The immediate operand of the `HVC`.
        imm: u16,
    },
    /// `SMC #imm16`. With the device tree declaring the `hvc` conduit, an
    /// `SMC` from the guest is not a PSCI call and has to be faulted back into
    /// the guest as undefined.
    Smc {
        /// The immediate operand of the `SMC`.
        imm: u16,
    },
    /// A trapped system register access.
    SysReg(SysReg),
    /// A load or store to a device.
    Mmio(MmioAccess),
    /// Anything else: an instruction abort, or an exception class this
    /// backend does not emulate (a stage-2 fault the guest caused, a debug
    /// exception). The backend reports it and stops the VM rather than
    /// guessing.
    Unhandled {
        /// The exception class, as [`exception_class`] reports it.
        class: u8,
    },
}

impl Exception {
    /// Decodes an `ESR_EL2` value.
    pub const fn decode(syndrome: u64) -> Self {
        let class = exception_class(syndrome);
        let iss = syndrome & 0x01ff_ffff;
        match class {
            EC_WFX_TRAP => Exception::Wait(Wait {
                event: iss & WFX_ISS_IS_WFE != 0,
            }),
            EC_AA64_HVC => Exception::Hvc {
                imm: (iss & 0xffff) as u16,
            },
            EC_AA64_SMC => Exception::Smc {
                imm: (iss & 0xffff) as u16,
            },
            EC_SYSREG_TRAP => Exception::SysReg(SysReg::decode(iss)),
            EC_DATAABORT_LOWER => Exception::Mmio(MmioAccess::decode(iss)),
            class => Exception::Unhandled { class },
        }
    }

    /// Whether the handler has to advance the guest's program counter by one
    /// instruction after dealing with the exit.
    ///
    /// A data abort and a trapped `MRS`/`MSR` leave the PC *at* the faulting
    /// instruction (the instruction did not execute, so the handler emulates
    /// it and then steps over it), and so does a `WFI`/`WFE` trap and an
    /// `SMC` - but an `HVC` is a call, so its exception return address already
    /// points past it. Getting this wrong silently skips or repeats one
    /// instruction of the guest.
    pub const fn advances_pc(&self) -> bool {
        match self {
            Exception::Hvc { .. } | Exception::Unhandled { .. } => false,
            Exception::Wait(_)
            | Exception::Smc { .. }
            | Exception::SysReg(_)
            | Exception::Mmio(_) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hvf::gic::{ICC_IAR1_EL1, ICC_PMR_EL1};

    /// Builds an `ESR_EL2` from an exception class and instruction syndrome.
    fn esr(class: u8, iss: u64) -> u64 {
        (u64::from(class) << 26) | (1 << 25) /* IL */ | (iss & 0x01ff_ffff)
    }

    /// Builds the syndrome of a trapped `MRS`/`MSR`.
    fn sysreg_iss(read: bool, rt: u64, op0: u64, op1: u64, crn: u64, crm: u64, op2: u64) -> u64 {
        (op0 << 20)
            | (op2 << 17)
            | (op1 << 14)
            | (crn << 10)
            | (crm << 1)
            | u64::from(read)
            | (rt << 5)
    }

    #[test]
    fn exception_class_is_read_from_the_top_bits() {
        assert_eq!(exception_class(esr(EC_DATAABORT_LOWER, 0)), 0x24);
        assert_eq!(exception_class(esr(EC_SYSREG_TRAP, 0)), 0x18);
        assert_eq!(exception_class(esr(EC_WFX_TRAP, 0)), 0x01);
    }

    #[test]
    fn a_wfi_and_a_wfe_are_told_apart() {
        assert_eq!(
            Exception::decode(esr(EC_WFX_TRAP, 0)),
            Exception::Wait(Wait { event: false })
        );
        assert_eq!(
            Exception::decode(esr(EC_WFX_TRAP, WFX_ISS_IS_WFE)),
            Exception::Wait(Wait { event: true })
        );
        assert!(Exception::decode(esr(EC_WFX_TRAP, 0)).advances_pc());
    }

    #[test]
    fn psci_conduit_calls_do_not_advance_the_program_counter() {
        // `HVC #0` and `HVC #0x1234`.
        let hvc = Exception::decode(esr(EC_AA64_HVC, 0x1234));
        assert_eq!(hvc, Exception::Hvc { imm: 0x1234 });
        assert!(!hvc.advances_pc(), "an HVC's return address is past it");

        // An `SMC` is not the conduit this port declares, but the decode and
        // the PC rule are still the ones a handler needs.
        let smc = Exception::decode(esr(EC_AA64_SMC, 7));
        assert_eq!(smc, Exception::Smc { imm: 7 });
        assert!(smc.advances_pc());
    }

    #[test]
    fn a_trapped_system_register_decodes_to_the_hypervisor_encoding() {
        // `MRS X0, ICC_IAR1_EL1` and `MSR ICC_PMR_EL1, X3`.
        let iar = Exception::decode(esr(EC_SYSREG_TRAP, sysreg_iss(true, 0, 3, 0, 12, 12, 0)));
        assert_eq!(
            iar,
            Exception::SysReg(SysReg {
                read: true,
                rt: 0,
                reg: ICC_IAR1_EL1,
            })
        );
        assert!(iar.advances_pc());

        let pmr = Exception::decode(esr(EC_SYSREG_TRAP, sysreg_iss(false, 3, 3, 0, 4, 6, 0)));
        assert_eq!(
            pmr,
            Exception::SysReg(SysReg {
                read: false,
                rt: 3,
                reg: ICC_PMR_EL1,
            })
        );
    }

    #[test]
    fn the_hypervisor_encoding_matches_the_known_register_values() {
        // Values from `Hypervisor/hv.h`: `HV_SYS_REG_MIDR_EL1` = 0xc000,
        // `HV_SYS_REG_MPIDR_EL1` = 0xc005, `HV_SYS_REG_SCTLR_EL1` = 0xc080,
        // `HV_SYS_REG_CNTV_CTL_EL0` = 0xdf19 and `HV_SYS_REG_VBAR_EL1` = 0xc600.
        assert_eq!(sysreg(3, 0, 0, 0, 0), 0xc000);
        assert_eq!(sysreg(3, 0, 0, 0, 5), 0xc005);
        assert_eq!(sysreg(3, 0, 1, 0, 0), 0xc080);
        assert_eq!(sysreg(3, 3, 14, 3, 1), 0xdf19);
        assert_eq!(sysreg(3, 0, 12, 0, 0), 0xc600);
    }

    #[test]
    fn encode_then_decode_round_trips_every_field() {
        for op0 in 0..4u64 {
            for op1 in 0..8u64 {
                for crn in 0..16u64 {
                    for crm in [0u64, 3, 6, 9, 12, 15] {
                        for op2 in 0..8u64 {
                            let decoded =
                                SysReg::decode(sysreg_iss(false, 31, op0, op1, crn, crm, op2));
                            assert_eq!(
                                u64::from(decoded.reg),
                                u64::from(sysreg(
                                    op0 as u16, op1 as u16, crn as u16, crm as u16, op2 as u16
                                ))
                            );
                            assert!(!decoded.read);
                            assert_eq!(decoded.rt, 31);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_device_access_carries_the_width_the_register_and_the_direction() {
        // A store of a word from X1: SAS = 2 (four bytes), WnR set, ISV set.
        let write = MmioAccess::decode((1 << 24) | (2 << 22) | (1 << 16) | (1 << 6));
        assert_eq!(
            write,
            MmioAccess {
                read: false,
                rt: 1,
                len: 4,
                sign_extend: false,
                valid: true,
                cache_maintenance: false,
            }
        );

        // A sign-extended halfword load into X7: SAS = 1, SSE set, WnR clear.
        let read = MmioAccess::decode((1 << 24) | (1 << 22) | (1 << 21) | (7 << 16));
        assert_eq!(
            read,
            MmioAccess {
                read: true,
                rt: 7,
                len: 2,
                sign_extend: true,
                valid: true,
                cache_maintenance: false,
            }
        );

        // A byte access, and the widths of every SAS encoding.
        for sas in 0..4u64 {
            assert_eq!(MmioAccess::decode((1 << 24) | (sas << 22)).len, 1 << sas);
        }
    }

    #[test]
    fn a_simd_access_and_a_cache_maintenance_op_are_reported_as_such() {
        // ISV clear: the syndrome has no instruction decode.
        let simd = MmioAccess::decode(2 << 22);
        assert!(!simd.valid);

        // CM set: the handler skips the instruction.
        let cache = MmioAccess::decode((1 << 24) | (1 << 8));
        assert!(cache.cache_maintenance);
    }

    #[test]
    fn a_device_burst_is_decoded_as_an_mmio_access() {
        let decoded = Exception::decode(esr(EC_DATAABORT_LOWER, (1 << 24) | (3 << 22)));
        match decoded {
            Exception::Mmio(access) => {
                assert_eq!(access.len, 8);
                assert!(access.read);
            }
            other => panic!("expected an MMIO access, got {other:?}"),
        }
        assert!(decoded.advances_pc());
    }

    #[test]
    fn anything_else_is_unhandled_and_does_not_step_the_guest() {
        for class in [0x00u8, EC_INSNABORT_LOWER, 0x26, 0x3f] {
            let decoded = Exception::decode(esr(class, 0x1234));
            assert_eq!(decoded, Exception::Unhandled { class });
            assert!(!decoded.advances_pc());
        }
    }
}
