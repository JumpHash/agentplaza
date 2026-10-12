// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/arch/x86_64/{regs,gdt}.rs (Apache-2.0):
// the same GDT, page tables and register values, expressed as data instead of
// KVM ioctls.

//! x86_64 64-bit Linux boot state, described as plain data.
//!
//! The 64-bit boot protocol expects the CPU to already be in long mode when the
//! kernel starts: paging enabled with the first 1 GiB identity-mapped, flat code
//! and data segments, RSI pointing at the zero page, and RIP at the 64-bit entry
//! point. This module computes that state without any platform calls. The WHP
//! FFI layer turns [`BootRegisters`] into `WHvSetVirtualProcessorRegisters` calls
//! and writes each [`MemoryWrite`] into guest memory.

/// Guest-physical address of the GDT.
pub const BOOT_GDT_OFFSET: u64 = 0x500;
/// Guest-physical address of the (empty) IDT.
pub const BOOT_IDT_OFFSET: u64 = 0x520;
/// Guest-physical address of the PML4 table.
pub const PML4_START: u64 = 0x9000;
/// Guest-physical address of the page-directory-pointer table (PDPT).
pub const PDPTE_START: u64 = 0xa000;
/// Guest-physical address of the page directory (PD).
pub const PDE_START: u64 = 0xb000;
/// Initial stack pointer for the boot vCPU.
pub const BOOT_STACK_POINTER: u64 = 0x8ff0;
/// Guest-physical address of the zero page (boot parameters), passed in RSI.
pub const ZERO_PAGE_START: u64 = 0x7000;

/// CR0.PE: protected mode enable.
pub const X86_CR0_PE: u64 = 0x1;
/// CR0.ET: extension type.
pub const X86_CR0_ET: u64 = 0x10;
/// CR0.PG: paging enable.
pub const X86_CR0_PG: u64 = 0x8000_0000;
/// CR4.PAE: physical address extension, required for long mode.
pub const X86_CR4_PAE: u64 = 0x20;
/// EFER.LME: long mode enable.
pub const EFER_LME: u64 = 0x100;
/// EFER.LMA: long mode active.
pub const EFER_LMA: u64 = 0x400;
/// The PAT MSR's power-on value.
pub const MSR_PAT_DEFAULT: u64 = 0x0007_0406_0007_0406;

/// Size of one page-table page in bytes.
const PAGE_SIZE: usize = 4096;

/// A segment register's hidden state: base, limit, selector and access rights.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Segment {
    /// Linear base address.
    pub base: u64,
    /// Segment limit (20 bits).
    pub limit: u32,
    /// Segment selector.
    pub selector: u16,
    /// Segment type (4 bits).
    pub type_: u8,
    /// Segment present bit.
    pub present: bool,
    /// Descriptor privilege level (0..=3).
    pub dpl: u8,
    /// Default operation size (D/B bit).
    pub db: bool,
    /// Descriptor type: true for code/data, false for system.
    pub s: bool,
    /// 64-bit code segment (L bit).
    pub l: bool,
    /// Granularity: limit is in 4 KiB units.
    pub g: bool,
    /// Available for system software.
    pub avl: bool,
}

impl Segment {
    /// The WHP (`WHV_X64_SEGMENT_REGISTER::Attributes`) encoding: bits 0..3 type, bit 4 S (non-system),
    /// bits 5..6 DPL, bit 7 present, bit 12 AVL, bit 13 L, bit 14 D/B, bit 15 G.
    pub fn attributes(&self) -> u16 {
        (u16::from(self.type_) & 0xf)
            | (u16::from(self.s) << 4)
            | ((u16::from(self.dpl) & 0x3) << 5)
            | (u16::from(self.present) << 7)
            | (u16::from(self.avl) << 12)
            | (u16::from(self.l) << 13)
            | (u16::from(self.db) << 14)
            | (u16::from(self.g) << 15)
    }

    /// Decodes a GDT entry the way Firecracker's `kvm_segment_from_gdt` does (selector = table_index * 8).
    pub fn from_gdt_entry(entry: u64, table_index: u8) -> Segment {
        Segment {
            base: ((entry >> 16) & 0xff_ffff) | (((entry >> 56) & 0xff) << 24),
            limit: ((entry & 0xffff) | (((entry >> 48) & 0xf) << 16)) as u32,
            selector: u16::from(table_index) * 8,
            type_: ((entry >> 40) & 0xf) as u8,
            s: bit(entry, 44),
            dpl: ((entry >> 45) & 0x3) as u8,
            present: bit(entry, 47),
            avl: bit(entry, 52),
            l: bit(entry, 53),
            db: bit(entry, 54),
            g: bit(entry, 55),
        }
    }
}

/// Firecracker's `gdt_entry(flags, base, limit)`: packs a 32-bit base and a 20-bit limit
/// together with the access and flag bits of `flags` into one 8-byte descriptor.
pub const fn gdt_entry(flags: u16, base: u32, limit: u32) -> u64 {
    ((((base as u64) & 0xff00_0000) << (56 - 24)))
        | (((flags as u64) & 0x0000_f0ff) << 40)
        | (((limit as u64) & 0x000f_0000) << (48 - 16))
        | (((base as u64) & 0x00ff_ffff) << 16)
        | ((limit as u64) & 0x0000_ffff)
}

/// A descriptor-table register (GDTR or IDTR): base address and byte limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableRegister {
    /// Guest-physical base address of the table.
    pub base: u64,
    /// Table size in bytes, minus one.
    pub limit: u16,
}

/// Architectural register state for the boot vCPU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootRegisters {
    /// Instruction pointer: the 64-bit kernel entry point.
    pub rip: u64,
    /// Stack pointer.
    pub rsp: u64,
    /// Frame pointer.
    pub rbp: u64,
    /// Zero-page pointer, per the 64-bit boot protocol.
    pub rsi: u64,
    /// Flags register; only the reserved bit 1 is set.
    pub rflags: u64,
    /// Control register 0 (protected mode, extension type, paging).
    pub cr0: u64,
    /// Control register 3: guest-physical address of the PML4 table.
    pub cr3: u64,
    /// Control register 4 (PAE).
    pub cr4: u64,
    /// EFER MSR (long mode enable and active).
    pub efer: u64,
    /// PAT MSR.
    pub pat: u64,
    /// Code segment register.
    pub cs: Segment,
    /// Data segment register.
    pub ds: Segment,
    /// Extra segment register.
    pub es: Segment,
    /// FS segment register.
    pub fs: Segment,
    /// GS segment register.
    pub gs: Segment,
    /// Stack segment register.
    pub ss: Segment,
    /// Task register, pointing at the TSS descriptor.
    pub tr: Segment,
    /// Local descriptor table register, unused.
    pub ldtr: Segment,
    /// Global descriptor table register.
    pub gdtr: TableRegister,
    /// Interrupt descriptor table register.
    pub idtr: TableRegister,
}

/// Bytes the boot state needs in guest memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryWrite {
    /// Guest-physical address to write at.
    pub gpa: u64,
    /// Bytes to write.
    pub bytes: Vec<u8>,
}

/// The register state and memory contents for entering a 64-bit kernel at `entry`.
pub fn long_mode_boot(entry: u64) -> (BootRegisters, Vec<MemoryWrite>) {
    // Null descriptor, 64-bit code, flat data, and a TSS descriptor.
    let gdt = [
        0,
        gdt_entry(0xa09b, 0, 0xfffff),
        gdt_entry(0xc093, 0, 0xfffff),
        gdt_entry(0x808b, 0, 0xfffff),
    ];
    let mut gdt_bytes = Vec::with_capacity(gdt.len() * 8);
    for descriptor in gdt.iter() {
        gdt_bytes.extend_from_slice(&descriptor.to_le_bytes());
    }

    let data = Segment::from_gdt_entry(gdt[2], 2);
    let regs = BootRegisters {
        rip: entry,
        rsp: BOOT_STACK_POINTER,
        rbp: BOOT_STACK_POINTER,
        rsi: ZERO_PAGE_START,
        rflags: 0x2,
        cr0: X86_CR0_PE | X86_CR0_ET | X86_CR0_PG,
        cr3: PML4_START,
        cr4: X86_CR4_PAE,
        efer: EFER_LME | EFER_LMA,
        pat: MSR_PAT_DEFAULT,
        cs: Segment::from_gdt_entry(gdt[1], 1),
        ds: data,
        es: data,
        fs: data,
        gs: data,
        ss: data,
        tr: Segment::from_gdt_entry(gdt[3], 3),
        ldtr: Segment {
            selector: 0,
            base: 0,
            limit: 0xffff,
            type_: 2,
            present: true,
            ..Default::default()
        },
        gdtr: TableRegister {
            base: BOOT_GDT_OFFSET,
            limit: 4 * 8 - 1,
        },
        idtr: TableRegister {
            base: BOOT_IDT_OFFSET,
            limit: 7,
        },
    };

    let writes = vec![
        MemoryWrite {
            gpa: BOOT_GDT_OFFSET,
            bytes: gdt_bytes,
        },
        MemoryWrite {
            gpa: BOOT_IDT_OFFSET,
            bytes: vec![0u8; 8],
        },
        MemoryWrite {
            gpa: PML4_START,
            bytes: table_page(|i| if i == 0 { PDPTE_START | 0x03 } else { 0 }),
        },
        MemoryWrite {
            gpa: PDPTE_START,
            bytes: table_page(|i| if i == 0 { PDE_START | 0x03 } else { 0 }),
        },
        // 512 entries of 2 MiB pages identity-map the first 1 GiB.
        MemoryWrite {
            gpa: PDE_START,
            bytes: table_page(|i| (i << 21) | 0x83),
        },
    ];

    (regs, writes)
}

/// Bit `n` of `entry` as a bool.
fn bit(entry: u64, n: u32) -> bool {
    ((entry >> n) & 1) == 1
}

/// One page-table page: 512 little-endian entries, where entry `i` is `entry(i)`.
fn table_page(entry: impl Fn(u64) -> u64) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(PAGE_SIZE);
    for index in 0..(PAGE_SIZE as u64 / 8) {
        bytes.extend_from_slice(&entry(index).to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTRY: u64 = 0x0010_0000;

    fn write_at(writes: &[MemoryWrite], gpa: u64) -> &[u8] {
        &writes
            .iter()
            .find(|w| w.gpa == gpa)
            .expect("write exists")
            .bytes
    }

    fn entry_at(bytes: &[u8], index: usize) -> u64 {
        let mut raw = [0u8; 8];
        raw.copy_from_slice(&bytes[index * 8..index * 8 + 8]);
        u64::from_le_bytes(raw)
    }

    #[test]
    fn gdt_entries_match_firecracker_values() {
        assert_eq!(gdt_entry(0xa09b, 0, 0xfffff), 0x00af_9b00_0000_ffff);
        assert_eq!(gdt_entry(0xc093, 0, 0xfffff), 0x00cf_9300_0000_ffff);
        assert_eq!(gdt_entry(0x808b, 0, 0xfffff), 0x008f_8b00_0000_ffff);
    }

    #[test]
    fn code_segment_is_flat_64bit_code() {
        let (regs, _) = long_mode_boot(ENTRY);
        let cs = regs.cs;
        assert!(cs.l);
        assert!(!cs.db);
        assert_eq!(cs.type_, 0xb);
        assert!(cs.s);
        assert!(cs.present);
        assert!(cs.g);
        assert!(!cs.avl);
        assert_eq!(cs.selector, 8);
        assert_eq!(cs.limit, 0xfffff);
        assert_eq!(cs.base, 0);
        assert_eq!(cs.dpl, 0);
    }

    #[test]
    fn data_segments_are_flat_32bit_data() {
        let (regs, _) = long_mode_boot(ENTRY);
        for seg in [regs.ds, regs.es, regs.fs, regs.gs, regs.ss].iter() {
            assert!(seg.db);
            assert!(!seg.l);
            assert_eq!(seg.type_, 3);
            assert_eq!(seg.selector, 0x10);
            assert!(seg.s && seg.present && seg.g);
            assert_eq!(seg.limit, 0xfffff);
            assert_eq!(seg.base, 0);
        }
    }

    #[test]
    fn task_register_points_at_tss_descriptor() {
        let (regs, _) = long_mode_boot(ENTRY);
        assert_eq!(regs.tr.selector, 0x18);
        assert_eq!(regs.tr.type_, 0xb);
        assert!(!regs.tr.s);
        assert!(regs.tr.present);
        assert!(regs.tr.g);
        assert!(!regs.tr.db);
        assert!(!regs.tr.l);
    }

    #[test]
    fn attributes_match_gdt_flag_words() {
        let (regs, _) = long_mode_boot(ENTRY);
        assert_eq!(regs.cs.attributes(), 0xa09b);
        assert_eq!(regs.ds.attributes(), 0xc093);
        assert_eq!(regs.tr.attributes(), 0x808b);
        assert_eq!(regs.ldtr.attributes(), 0x82);
    }

    #[test]
    fn identity_page_tables_map_first_gib() {
        let (_, writes) = long_mode_boot(ENTRY);
        let pml4 = write_at(&writes, PML4_START);
        let pdpt = write_at(&writes, PDPTE_START);
        let pd = write_at(&writes, PDE_START);
        assert_eq!(pml4.len(), 4096);
        assert_eq!(pdpt.len(), 4096);
        assert_eq!(pd.len(), 4096);

        assert_eq!(entry_at(pml4, 0), 0xa003);
        assert_eq!(entry_at(pml4, 1), 0);
        assert_eq!(entry_at(pdpt, 0), 0xb003);
        assert_eq!(entry_at(pdpt, 1), 0);
        assert_eq!(entry_at(pd, 0), 0x83);
        assert_eq!(entry_at(pd, 1), 0x0020_0083);
        assert_eq!(entry_at(pd, 511), (511 << 21) | 0x83);
        for i in 0..512u64 {
            assert_eq!(entry_at(pd, i as usize), (i << 21) | 0x83);
        }
    }

    #[test]
    fn registers_enter_long_mode_at_entry() {
        let (regs, _) = long_mode_boot(ENTRY);
        assert_eq!(regs.rip, ENTRY);
        assert_eq!(regs.rsp, 0x8ff0);
        assert_eq!(regs.rbp, 0x8ff0);
        assert_eq!(regs.rsi, 0x7000);
        assert_eq!(regs.rflags, 0x2);
        // PE | ET | PG
        assert_eq!(regs.cr0, 0x8000_0011);
        assert_eq!(regs.cr3, 0x9000);
        assert_eq!(regs.cr4, 0x20);
        assert_eq!(regs.efer, 0x500);
        assert_eq!(regs.pat, 0x0007_0406_0007_0406);
        assert_eq!(
            regs.gdtr,
            TableRegister {
                base: 0x500,
                limit: 31
            }
        );
        assert_eq!(
            regs.idtr,
            TableRegister {
                base: 0x520,
                limit: 7
            }
        );
    }

    #[test]
    fn memory_writes_are_in_documented_order() {
        let (_, writes) = long_mode_boot(ENTRY);
        let gpas: Vec<u64> = writes.iter().map(|w| w.gpa).collect();
        assert_eq!(gpas, vec![0x500, 0x520, 0x9000, 0xa000, 0xb000]);
    }

    #[test]
    fn gdt_bytes_are_little_endian_at_gdt_offset() {
        let (_, writes) = long_mode_boot(ENTRY);
        let gdt = write_at(&writes, BOOT_GDT_OFFSET);
        assert_eq!(gdt.len(), 32);
        assert_eq!(entry_at(gdt, 0), 0);
        assert_eq!(entry_at(gdt, 1), 0x00af_9b00_0000_ffff);
        assert_eq!(entry_at(gdt, 2), 0x00cf_9300_0000_ffff);
        assert_eq!(entry_at(gdt, 3), 0x008f_8b00_0000_ffff);
        assert_eq!(&gdt[8..16], &0x00af_9b00_0000_ffffu64.to_le_bytes()[..]);
        assert_eq!(write_at(&writes, BOOT_IDT_OFFSET), &[0u8; 8][..]);
    }
}
