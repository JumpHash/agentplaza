// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//! The platform-independent half of the Windows arm64 WHP backend (`docs/windows-whp-arm64.md`): the
//! register names, the exit context a `WHvRunVirtualProcessor` call fills, what a memory exit means
//! for the guest, the reset mapping, and the bytes of the two structures the backend hands WHP (the
//! GIC partition property and the interrupt request). It is compiled and unit-tested on every host;
//! only [`super::vm`] and [`super::vcpu`] call the Windows API.
//!
//! **Where the numbers come from.** The Windows SDK header `WinHvPlatformDefs.h` (the arm64 section
//! of `Microsoft.Windows.SDK` `um/WinHvPlatformDefs.h`, as published with the SDK) is the authority;
//! `windows-sys` 0.61 (the workspace's copy) carries the saved-state `ARM64_Register*` IDs, which are a
//! different ID space, and none of the WHP arm64 exit or interrupt layouts. So the values below are the
//! ones the design doc records, each with its offset; they are checked against the header on a
//! Windows-on-Arm machine before a boot is claimed (see the doc's "Status").

use crate::hvf::exit::{Exception, MmioAccess};
use crate::hvf::gic::{GICV3_REDIST_BASE, GICV3_REDIST_STRIDE};

/// `WHvPartitionPropertyCodeArm64IcParameters`: the GIC the hypervisor emulates.
pub const PROPERTY_ARM64_IC_PARAMETERS: u32 = 0x1012;
/// `WHvArm64IcEmulationModeGicV3`.
pub const IC_EMULATION_GICV3: u32 = 1;
/// The GIC's LPI INTID bits (`GicLpiIntIdBits`): the design doc's value.
pub const GIC_LPI_INTID_BITS: u32 = 1;
/// The PPI of the CNTV overflow and of the PMU interrupt, as the design doc records them.
pub const GIC_PPI_OVERFLOW_CNTV: u32 = 0x14;
pub const GIC_PPI_PERFORMANCE_MONITORS: u32 = 0x17;
/// Bytes of the `IcParameters` property buffer the backend passes (zero padded).
pub const IC_PARAMETERS_SIZE: usize = 48;

/// `WHvArm64RegisterX0`; X0..X28 follow at +1 each.
pub const REGISTER_X0: u32 = 0x0002_0000;
/// `WHvArm64RegisterFp` (X29).
pub const REGISTER_FP: u32 = 0x0002_001d;
/// `WHvArm64RegisterLr` (X30).
pub const REGISTER_LR: u32 = 0x0002_001e;
/// `WHvArm64RegisterSp`.
pub const REGISTER_SP: u32 = 0x0002_001f;
/// `WHvArm64RegisterPc`.
pub const REGISTER_PC: u32 = 0x0002_0022;
/// `WHvArm64RegisterPstate`.
pub const REGISTER_PSTATE: u32 = 0x0002_0023;
/// `WHvArm64RegisterMpidrEl1`.
pub const REGISTER_MPIDR_EL1: u32 = 0x0004_0001;
/// `WHvArm64RegisterGicrBaseGpa`: a vCPU's redistributor base.
pub const REGISTER_GICR_BASE_GPA: u32 = 0x0006_3000;

/// The PSTATE a Linux arm64 kernel is entered with: EL1h, DAIF masked (`0x3c5`).
pub const BOOT_PSTATE: u64 = 0x3c5;

/// The exit reasons the arm64 run loop acts on (the design doc's table).
pub const EXIT_UNMAPPED_GPA: u32 = 0x8000_0000;
pub const EXIT_GPA_INTERCEPT: u32 = 0x8000_0001;
pub const EXIT_HYPERCALL: u32 = 0x8000_0050;
pub const EXIT_ARM64_RESET: u32 = 0x8001_000c;
pub const EXIT_CANCELED: u32 = 0xffff_ffff;

/// `WHV_INTERRUPT_CONTROL` bit 34: asserts the line (clear deasserts it).
pub const INTERRUPT_ASSERTED: u64 = 1 << 34;

/// Bytes of the exit context: `ExitReason`, `Reserved`, `Reserved1`, then the 256-byte message union.
pub const EXIT_CONTEXT_SIZE: usize = 16 + 256;
/// Where the message union starts in the exit context.
const MESSAGE: usize = 16;
/// The header's `pc` field.
const HEADER_PC: usize = 8;
/// The header's `cpsr` field.
const HEADER_CPSR: usize = 16;
/// The memory intercept's syndrome (ESR), after the header, the cache type and the instruction bytes.
const MEMORY_GPA: usize = 0x30;
const MEMORY_SYNDROME: usize = 0x38;
/// The reset intercept's type and code, after the header.
const RESET_TYPE: usize = 0x18;
const RESET_CODE: usize = 0x1c;
/// The hypervisor's reset kinds.
pub const RESET_OFF: u32 = 0;
pub const RESET_REBOOT: u32 = 1;
pub const RESET_SYSTEM: u32 = 2;
pub const RESET_HIBERNATE: u32 = 3;

/// The vCPU's MPIDR for `index`: the affinity-0 field is the index and bit 31 is set, as the device
/// tree's `cpu` nodes name it.
pub const fn mpidr(index: u32) -> u64 {
    (1 << 31) | index as u64
}

/// The redistributor base of vCPU `index`: the GIC's redistributor window, one 128 KiB frame per vCPU.
pub const fn redistributor_base(index: u32) -> u64 {
    GICV3_REDIST_BASE + index as u64 * GICV3_REDIST_STRIDE
}

/// What one exit means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArmExit {
    /// A load or store to an address with no memory behind it: a device access.
    Mmio(Mmio),
    /// The guest stopped (`RESET_OFF`) or restarted (`RESET_REBOOT`, `RESET_SYSTEM`) the VM.
    Reset { kind: u32, code: u32 },
    /// `WHvCancelRunVirtualProcessor` ended the run: the vCPU checks its stop flag and runs again.
    Canceled,
    /// An exit the backend does not emulate (a hypercall, which the hypervisor handles itself for
    /// PSCI; an instruction abort; a syndrome without a valid instruction decode). The VM is stopped
    /// rather than guessed at. `reason` names the exit or the exception class.
    Unsupported { reason: String },
}

/// One device access, decoded from a memory intercept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mmio {
    /// The guest physical address accessed.
    pub gpa: u64,
    /// The PC of the faulting instruction; the completion moves it past the instruction.
    pub pc: u64,
    /// The access itself (the same decoder the Hypervisor.framework backend uses).
    pub access: MmioAccess,
    /// `SF`: a 64-bit register is the transfer register (a 32-bit access zeroes the top half).
    pub is_64bit: bool,
}

/// Decodes the exit context the hypervisor filled. `context` is at least [`EXIT_CONTEXT_SIZE`] bytes.
pub fn decode_exit(context: &[u8]) -> ArmExit {
    let reason = read_u32(context, 0);
    let message = &context[MESSAGE..];
    match reason {
        EXIT_UNMAPPED_GPA | EXIT_GPA_INTERCEPT => decode_memory(message),
        EXIT_ARM64_RESET => ArmExit::Reset {
            kind: read_u32(message, RESET_TYPE),
            code: read_u32(message, RESET_CODE),
        },
        EXIT_CANCELED => ArmExit::Canceled,
        EXIT_HYPERCALL => ArmExit::Unsupported {
            reason: "a hypercall: PSCI belongs to the hypervisor, so the guest made an HVC the backend does not serve"
                .to_string(),
        },
        other => ArmExit::Unsupported {
            reason: format!("the exit reason {other:#010x}"),
        },
    }
}

fn decode_memory(message: &[u8]) -> ArmExit {
    let pc = read_u64(message, HEADER_PC);
    let _cpsr = read_u64(message, HEADER_CPSR);
    let gpa = read_u64(message, MEMORY_GPA);
    let syndrome = read_u64(message, MEMORY_SYNDROME);
    match Exception::decode(syndrome) {
        Exception::Mmio(access) if access.valid => ArmExit::Mmio(Mmio {
            gpa,
            pc,
            access,
            is_64bit: is_64bit(syndrome),
        }),
        Exception::Mmio(_) => ArmExit::Unsupported {
            reason: format!(
                "a data abort at {gpa:#x} without a valid instruction decode (a SIMD or exclusive access)"
            ),
        },
        other => ArmExit::Unsupported {
            reason: format!("a memory intercept at {gpa:#x} that is not a data abort: {other:?}"),
        },
    }
}

/// `SF` of a data abort's syndrome (bit 15 of the instruction syndrome).
pub const fn is_64bit(syndrome: u64) -> bool {
    (syndrome >> 15) & 1 == 1
}

/// What a completed read leaves in its transfer register, from the bytes the bus returned.
///
/// The value is the little-endian bytes zero-extended; `SSE` sign-extends from the access width, and
/// a 32-bit transfer register (`SF` clear) keeps only its low half.
pub fn read_result(bytes: &[u8], access: MmioAccess, is_64bit: bool) -> u64 {
    let width = usize::from(access.len).min(8);
    let mut value = 0u64;
    for (index, byte) in bytes.iter().take(width).enumerate() {
        value |= u64::from(*byte) << (8 * index);
    }
    let bits = 8 * width as u32;
    if access.sign_extend && bits < 64 && (value >> (bits - 1)) & 1 == 1 {
        value |= u64::MAX << bits;
    }
    if is_64bit { value } else { value & 0xffff_ffff }
}

/// The bytes a store writes: the transfer register's low `len` bytes, little-endian.
pub fn write_bytes(value: u64, access: MmioAccess) -> [u8; 8] {
    let mut bytes = value.to_le_bytes();
    for byte in bytes.iter_mut().skip(usize::from(access.len).min(8)) {
        *byte = 0;
    }
    bytes
}

/// The reset kind's meaning for the run loop.
pub const fn reset_stops_vm(kind: u32) -> Option<ResetOutcome> {
    match kind {
        RESET_OFF => Some(ResetOutcome::Off),
        RESET_REBOOT | RESET_SYSTEM => Some(ResetOutcome::Reset),
        _ => None,
    }
}

/// What a guest reset ends the VM as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetOutcome {
    /// The guest shut the VM down.
    Off,
    /// The guest asked for a reset.
    Reset,
}

/// The `WHV_INTERRUPT_CONTROL` the arm64 layout describes, for SPI `intid` (32..), asserted or not.
///
/// Layout: `TargetPartition` (u64, 0), `InterruptControl` (u64, asserted in bit 34), `DestinationAddress`
/// (u64, 0: routed by the GIC's `IROUTER`), `RequestedVector` (u32, the INTID), `TargetVtl` (u8, 0),
/// two reserved bytes. 32 bytes.
pub fn interrupt_control(intid: u32, asserted: bool) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    let control = if asserted { INTERRUPT_ASSERTED } else { 0 };
    bytes[8..16].copy_from_slice(&control.to_le_bytes());
    bytes[24..28].copy_from_slice(&intid.to_le_bytes());
    bytes
}

/// The `WHvPartitionPropertyCodeArm64IcParameters` buffer for a GICv3 whose distributor is at `gicd`:
/// `EmulationMode` (u32, 0), `Reserved` (u32), `GicdBaseAddress` (u64, 8), `GitsTranslatorBaseAddress`
/// (u64, 0, no ITS), then the LPI and PPI fields the design doc lists, zero padded to
/// [`IC_PARAMETERS_SIZE`].
pub fn ic_parameters(gicd: u64) -> [u8; IC_PARAMETERS_SIZE] {
    let mut bytes = [0u8; IC_PARAMETERS_SIZE];
    bytes[0..4].copy_from_slice(&IC_EMULATION_GICV3.to_le_bytes());
    bytes[8..16].copy_from_slice(&gicd.to_le_bytes());
    bytes[28..32].copy_from_slice(&GIC_LPI_INTID_BITS.to_le_bytes());
    bytes[32..36].copy_from_slice(&GIC_PPI_OVERFLOW_CNTV.to_le_bytes());
    bytes[36..40].copy_from_slice(&GIC_PPI_PERFORMANCE_MONITORS.to_le_bytes());
    bytes
}

/// The registers vCPU 0 is entered with: X0 = the device tree, PC = the kernel entry, PSTATE = EL1h
/// with DAIF masked. Every vCPU also gets its MPIDR and redistributor base; those are [`vcpu_registers`].
pub fn boot_registers(entry: u64, fdt: u64) -> [(u32, u64); 3] {
    [
        (REGISTER_X0, fdt),
        (REGISTER_PC, entry),
        (REGISTER_PSTATE, BOOT_PSTATE),
    ]
}

/// The registers every vCPU is created with: its MPIDR and its redistributor base.
pub fn vcpu_registers(index: u32) -> [(u32, u64); 2] {
    [
        (REGISTER_MPIDR_EL1, mpidr(index)),
        (REGISTER_GICR_BASE_GPA, redistributor_base(index)),
    ]
}

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

fn read_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("eight bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hvf::gic::GICV3_DIST_BASE;

    /// A synthetic exit context: the reason, then a message header and the memory fields.
    fn context(reason: u32, pc: u64, gpa: u64, syndrome: u64) -> Vec<u8> {
        let mut bytes = vec![0u8; EXIT_CONTEXT_SIZE];
        bytes[0..4].copy_from_slice(&reason.to_le_bytes());
        let m = MESSAGE;
        bytes[m + HEADER_PC..m + HEADER_PC + 8].copy_from_slice(&pc.to_le_bytes());
        bytes[m + MEMORY_GPA..m + MEMORY_GPA + 8].copy_from_slice(&gpa.to_le_bytes());
        bytes[m + MEMORY_SYNDROME..m + MEMORY_SYNDROME + 8]
            .copy_from_slice(&syndrome.to_le_bytes());
        bytes
    }

    /// An ESR for a data abort with a valid decode: `len_log2` (SAS), `sign`, `rt`, `write`, `sf`.
    fn data_abort(len_log2: u64, sign: bool, rt: u64, write: bool, sf: bool) -> u64 {
        let mut iss = 1u64 << 24; // ISV
        iss |= len_log2 << 22;
        iss |= u64::from(sign) << 21;
        iss |= rt << 16;
        iss |= u64::from(write) << 6;
        iss |= u64::from(sf) << 15;
        (u64::from(EC_DATAABORT_LOWER) << 26) | iss
    }

    const EC_DATAABORT_LOWER: u8 = 0x24;

    #[test]
    fn the_register_ids_are_the_design_doc_values() {
        assert_eq!(REGISTER_X0, 0x0002_0000);
        assert_eq!(REGISTER_PC, 0x0002_0022);
        assert_eq!(REGISTER_PSTATE, 0x0002_0023);
        assert_eq!(REGISTER_MPIDR_EL1, 0x0004_0001);
        assert_eq!(REGISTER_GICR_BASE_GPA, 0x0006_3000);
        // X1..X28 follow X0 one by one: X(n) = X0 + n.
        assert_eq!(REGISTER_X0 + 28, REGISTER_FP - 1);
    }

    #[test]
    fn each_vcpu_has_its_redistributor_frame_and_its_affinity() {
        assert_eq!(redistributor_base(0), GICV3_REDIST_BASE);
        assert_eq!(redistributor_base(1), GICV3_REDIST_BASE + 0x2_0000);
        assert_eq!(redistributor_base(3), GICV3_REDIST_BASE + 3 * 0x2_0000);
        assert_eq!(mpidr(0), 1 << 31);
        assert_eq!(mpidr(5), (1 << 31) | 5);
        assert_eq!(
            vcpu_registers(2),
            [
                (REGISTER_MPIDR_EL1, mpidr(2)),
                (REGISTER_GICR_BASE_GPA, redistributor_base(2))
            ]
        );
    }

    #[test]
    fn vcpu_zero_is_entered_with_the_fdt_in_x0_and_el1h_masked() {
        assert_eq!(
            boot_registers(0x8008_0000, 0x8f00_0000),
            [
                (REGISTER_X0, 0x8f00_0000),
                (REGISTER_PC, 0x8008_0000),
                (REGISTER_PSTATE, 0x3c5)
            ]
        );
    }

    #[test]
    fn a_load_decodes_to_a_read_into_its_register_at_its_width() {
        // LDR w3 from 0x0900_0000: SAS 2 (four bytes), no sign, SRT 3, a read, 32-bit.
        let syndrome = data_abort(2, false, 3, false, false);
        let exit = decode_exit(&context(
            EXIT_UNMAPPED_GPA,
            0x8000_1000,
            0x0900_0000,
            syndrome,
        ));
        match exit {
            ArmExit::Mmio(mmio) => {
                assert_eq!(mmio.gpa, 0x0900_0000);
                assert_eq!(mmio.pc, 0x8000_1000);
                assert!(mmio.access.read);
                assert_eq!(mmio.access.rt, 3);
                assert_eq!(mmio.access.len, 4);
                assert!(!mmio.is_64bit);
            }
            other => panic!("expected an MMIO exit, got {other:?}"),
        }
    }

    #[test]
    fn a_store_decodes_to_a_write_and_a_gpa_intercept_is_the_same_access() {
        let syndrome = data_abort(3, false, 10, true, true);
        let exit = decode_exit(&context(EXIT_GPA_INTERCEPT, 0x9000, 0x0a00_0000, syndrome));
        let ArmExit::Mmio(mmio) = exit else {
            panic!("expected MMIO, got {exit:?}")
        };
        assert!(!mmio.access.read);
        assert_eq!(mmio.access.rt, 10);
        assert_eq!(mmio.access.len, 8);
        assert!(mmio.is_64bit);
    }

    #[test]
    fn an_access_without_a_valid_decode_is_refused_not_emulated() {
        let syndrome = (u64::from(EC_DATAABORT_LOWER) << 26) | (1 << 6); // ISV clear, a write
        let exit = decode_exit(&context(EXIT_UNMAPPED_GPA, 0x9000, 0x0900_0000, syndrome));
        assert!(matches!(exit, ArmExit::Unsupported { .. }), "{exit:?}");
    }

    #[test]
    fn a_reset_names_its_kind_and_code() {
        let mut bytes = context(EXIT_ARM64_RESET, 0, 0, 0);
        bytes[MESSAGE + RESET_TYPE..MESSAGE + RESET_TYPE + 4]
            .copy_from_slice(&RESET_REBOOT.to_le_bytes());
        bytes[MESSAGE + RESET_CODE..MESSAGE + RESET_CODE + 4].copy_from_slice(&7u32.to_le_bytes());
        assert_eq!(
            decode_exit(&bytes),
            ArmExit::Reset {
                kind: RESET_REBOOT,
                code: 7
            }
        );
        assert_eq!(reset_stops_vm(RESET_OFF), Some(ResetOutcome::Off));
        assert_eq!(reset_stops_vm(RESET_SYSTEM), Some(ResetOutcome::Reset));
        assert_eq!(reset_stops_vm(RESET_HIBERNATE), None);
    }

    #[test]
    fn a_cancel_and_an_unknown_or_hypercall_exit_are_classified() {
        assert_eq!(
            decode_exit(&context(EXIT_CANCELED, 0, 0, 0)),
            ArmExit::Canceled
        );
        assert!(matches!(
            decode_exit(&context(EXIT_HYPERCALL, 0, 0, 0)),
            ArmExit::Unsupported { .. }
        ));
        assert!(matches!(
            decode_exit(&context(0x1234, 0, 0, 0)),
            ArmExit::Unsupported { .. }
        ));
    }

    #[test]
    fn a_load_fills_the_register_from_its_bytes_with_sign_and_width() {
        let word = data_abort(2, false, 0, false, false);
        let Exception::Mmio(access) = Exception::decode(word) else {
            unreachable!()
        };
        assert_eq!(
            read_result(&[0xff, 0xff, 0xff, 0xff], access, false),
            0xffff_ffff
        );

        let signed = data_abort(0, true, 0, false, true);
        let Exception::Mmio(access) = Exception::decode(signed) else {
            unreachable!()
        };
        assert_eq!(
            read_result(&[0x80], access, true),
            0xffff_ffff_ffff_ff80,
            "a byte sign-extends to 64 bits"
        );
        assert_eq!(
            read_result(&[0x80], access, false),
            0xffff_ff80,
            "a 32-bit transfer keeps the low half"
        );
    }

    #[test]
    fn a_store_writes_only_its_access_width() {
        let word = data_abort(1, false, 0, true, false);
        let Exception::Mmio(access) = Exception::decode(word) else {
            unreachable!()
        };
        assert_eq!(
            write_bytes(0x1122_3344_5566_7788, access)[..4],
            [0x88, 0x77, 0, 0]
        );
    }

    #[test]
    fn the_interrupt_request_names_the_spi_and_asserts_bit_34() {
        let asserted = interrupt_control(32 + 5, true);
        assert_eq!(asserted.len(), 32);
        assert_eq!(
            u64::from_le_bytes(asserted[8..16].try_into().unwrap()),
            1 << 34
        );
        assert_eq!(u32::from_le_bytes(asserted[24..28].try_into().unwrap()), 37);
        let cleared = interrupt_control(37, false);
        assert_eq!(u64::from_le_bytes(cleared[8..16].try_into().unwrap()), 0);
    }

    #[test]
    fn the_gic_property_carries_the_distributor_and_no_its() {
        let bytes = ic_parameters(GICV3_DIST_BASE);
        assert_eq!(
            u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            IC_EMULATION_GICV3
        );
        assert_eq!(
            u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            GICV3_DIST_BASE
        );
        assert_eq!(
            u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            0,
            "no ITS translator"
        );
        assert_eq!(bytes.len(), IC_PARAMETERS_SIZE);
    }
}
