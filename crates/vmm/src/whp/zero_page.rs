// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0

//! Linux x86 boot protocol "zero page": the `struct boot_params` (4096 bytes)
//! that the 64-bit kernel entry point reads, and the e820 memory map it carries.
//!
//! The layout follows `Documentation/arch/x86/zero-page.rst` and
//! `Documentation/arch/x86/boot.rst`. The setup header is copied from a bzImage
//! to offset `0x1f1`. The loader fields (`type_of_loader`, ramdisk, command
//! line, ACPI RSDP, `kernel_alignment`) and the e820 table are written at their
//! fixed offsets, little-endian. This mirrors what the KVM backend's
//! `configure_64bit_boot` writes through the `linux-loader` crate, without
//! depending on it.

/// Size of the zero page in bytes (one page).
pub const ZERO_PAGE_SIZE: usize = 4096;
/// e820 region type: usable RAM.
pub const E820_RAM: u32 = 1;
/// e820 region type: reserved, not usable by the guest OS.
pub const E820_RESERVED: u32 = 2;
/// Entries the zero page's e820 table holds.
pub const E820_MAX_ENTRIES: usize = 128;

const OFF_ACPI_RSDP_ADDR: usize = 0x070;
const OFF_E820_ENTRIES: usize = 0x1e8;
const OFF_SETUP_HEADER: usize = 0x1f1;
const OFF_BOOT_FLAG: usize = 0x1fe;
const OFF_HEADER: usize = 0x202;
const OFF_TYPE_OF_LOADER: usize = 0x210;
const OFF_RAMDISK_IMAGE: usize = 0x218;
const OFF_RAMDISK_SIZE: usize = 0x21c;
const OFF_CMD_LINE_PTR: usize = 0x228;
const OFF_KERNEL_ALIGNMENT: usize = 0x230;
const OFF_CMDLINE_SIZE: usize = 0x238;
const OFF_E820_TABLE: usize = 0x2d0;
const E820_ENTRY_SIZE: usize = 20;
/// End (exclusive) of the setup header area of the zero page.
const SETUP_HEADER_END: usize = 0x290;
/// Largest setup header that fits in the setup header area.
const SETUP_HEADER_MAX: usize = SETUP_HEADER_END - OFF_SETUP_HEADER;

const BOOT_FLAG: u16 = 0xaa55;
/// The "HdrS" signature the kernel checks at offset 0x202.
const HDR_MAGIC: u32 = 0x5372_6448;
/// `type_of_loader` value for a loader without an assigned ID.
const TYPE_OF_LOADER_UNASSIGNED: u8 = 0xff;
/// `kernel_alignment` for an ELF vmlinux, which has no setup header of its own.
const ELF_KERNEL_ALIGNMENT: u32 = 0x0100_0000;

const _: () = assert!(OFF_E820_TABLE + E820_MAX_ENTRIES * E820_ENTRY_SIZE <= ZERO_PAGE_SIZE);

/// One e820 memory map entry: a physical range and its type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct E820Entry {
    /// Start physical address of the range.
    pub addr: u64,
    /// Length of the range in bytes.
    pub size: u64,
    /// Range type: [`E820_RAM`], [`E820_RESERVED`], or another e820 type.
    pub kind: u32,
}

/// Errors from building the zero page.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ZeroPageError {
    /// The e820 table already holds [`E820_MAX_ENTRIES`] entries.
    #[error("the e820 map is full ({E820_MAX_ENTRIES} entries)")]
    E820Full,
    /// The setup header is longer than the 0x9f bytes of the setup header area.
    #[error("the setup header is {0} bytes; at most {max} fit", max = 0x290 - 0x1f1)]
    SetupHeaderTooLong(usize),
}

/// A Linux x86 zero page (`boot_params`) under construction.
pub struct ZeroPage {
    bytes: Box<[u8; ZERO_PAGE_SIZE]>,
}

impl ZeroPage {
    /// Creates a zero page.
    ///
    /// `setup_header` is the setup header of a bzImage: the bytes from image
    /// offset `0x1f1` through `0x202 + image[0x201]`. They are copied to offset
    /// `0x1f1`. `None` (an ELF vmlinux) leaves the header zero except for
    /// `kernel_alignment`, which is set to `0x0100_0000`.
    ///
    /// Returns [`ZeroPageError::SetupHeaderTooLong`] if the header does not fit
    /// in the setup header area.
    pub fn new(setup_header: Option<&[u8]>) -> Result<Self, ZeroPageError> {
        let mut zp = Self {
            bytes: Box::new([0; ZERO_PAGE_SIZE]),
        };
        match setup_header {
            Some(header) => {
                if header.len() > SETUP_HEADER_MAX {
                    return Err(ZeroPageError::SetupHeaderTooLong(header.len()));
                }
                zp.write_bytes(OFF_SETUP_HEADER, header);
            }
            None => zp.put_u32(OFF_KERNEL_ALIGNMENT, ELF_KERNEL_ALIGNMENT),
        }
        Ok(zp)
    }

    /// Sets the command line fields: `type_of_loader` = 0xff, `boot_flag` = 0xaa55,
    /// `header` = "HdrS", `cmd_line_ptr` = `addr` and `cmdline_size` = `size`.
    pub fn set_command_line(&mut self, addr: u32, size: u32) {
        self.bytes[OFF_TYPE_OF_LOADER] = TYPE_OF_LOADER_UNASSIGNED;
        self.write_bytes(OFF_BOOT_FLAG, &BOOT_FLAG.to_le_bytes());
        self.put_u32(OFF_HEADER, HDR_MAGIC);
        self.put_u32(OFF_CMD_LINE_PTR, addr);
        self.put_u32(OFF_CMDLINE_SIZE, size);
    }

    /// Sets the initial ramdisk (initrd) address and size.
    pub fn set_initrd(&mut self, addr: u32, size: u32) {
        self.put_u32(OFF_RAMDISK_IMAGE, addr);
        self.put_u32(OFF_RAMDISK_SIZE, size);
    }

    /// Sets the physical address of the ACPI RSDP.
    pub fn set_acpi_rsdp(&mut self, addr: u64) {
        self.put_u64(OFF_ACPI_RSDP_ADDR, addr);
    }

    /// Appends an entry to the e820 table.
    ///
    /// Returns [`ZeroPageError::E820Full`] once [`E820_MAX_ENTRIES`] entries are held.
    pub fn add_e820(&mut self, entry: E820Entry) -> Result<(), ZeroPageError> {
        let count = self.e820_count();
        if count >= E820_MAX_ENTRIES {
            return Err(ZeroPageError::E820Full);
        }
        let off = OFF_E820_TABLE + count * E820_ENTRY_SIZE;
        self.put_u64(off, entry.addr);
        self.put_u64(off + 8, entry.size);
        self.put_u32(off + 16, entry.kind);
        self.bytes[OFF_E820_ENTRIES] = (count + 1) as u8;
        Ok(())
    }

    /// Returns the e820 entries currently in the table, in order.
    pub fn e820_entries(&self) -> Vec<E820Entry> {
        (0..self.e820_count())
            .map(|i| {
                let off = OFF_E820_TABLE + i * E820_ENTRY_SIZE;
                E820Entry {
                    addr: self.get_u64(off),
                    size: self.get_u64(off + 8),
                    kind: self.get_u32(off + 16),
                }
            })
            .collect()
    }

    /// Returns all 4096 bytes of the zero page.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..]
    }

    fn e820_count(&self) -> usize {
        usize::from(self.bytes[OFF_E820_ENTRIES]).min(E820_MAX_ENTRIES)
    }

    fn write_bytes(&mut self, off: usize, src: &[u8]) {
        self.bytes[off..off + src.len()].copy_from_slice(src);
    }

    fn put_u32(&mut self, off: usize, value: u32) {
        self.write_bytes(off, &value.to_le_bytes());
    }

    fn put_u64(&mut self, off: usize, value: u64) {
        self.write_bytes(off, &value.to_le_bytes());
    }

    fn get_u32(&self, off: usize) -> u32 {
        let mut b = [0u8; 4];
        b.copy_from_slice(&self.bytes[off..off + 4]);
        u32::from_le_bytes(b)
    }

    fn get_u64(&self, off: usize) -> u64 {
        let mut b = [0u8; 8];
        b.copy_from_slice(&self.bytes[off..off + 8]);
        u64::from_le_bytes(b)
    }
}

/// The e820 map the KVM x86_64 backend writes, for guest RAM `regions` given as
/// `(start, len)` pairs.
///
/// The map is `[0, system_mem_start)` as RAM, then `[system_mem_start,
/// system_mem_start + system_mem_size)` as reserved, then
/// `[pci_mmconfig_start, pci_mmconfig_start + pci_mmconfig_size)` as reserved.
/// Each region is then added as RAM from `max(himem_start, start)` to its end.
/// A region that ends at or below `himem_start` is skipped.
pub fn standard_e820(
    regions: &[(u64, u64)],
    himem_start: u64,
    system_mem_start: u64,
    system_mem_size: u64,
    pci_mmconfig_start: u64,
    pci_mmconfig_size: u64,
) -> Vec<E820Entry> {
    let mut map = vec![
        E820Entry {
            addr: 0,
            size: system_mem_start,
            kind: E820_RAM,
        },
        E820Entry {
            addr: system_mem_start,
            size: system_mem_size,
            kind: E820_RESERVED,
        },
        E820Entry {
            addr: pci_mmconfig_start,
            size: pci_mmconfig_size,
            kind: E820_RESERVED,
        },
    ];
    for &(start, len) in regions {
        let end = start.saturating_add(len);
        if end <= himem_start {
            continue;
        }
        let addr = start.max(himem_start);
        map.push(E820Entry {
            addr,
            size: end - addr,
            kind: E820_RAM,
        });
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1 << 20;

    fn u32_at(bytes: &[u8], off: usize) -> u32 {
        u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap())
    }

    fn u64_at(bytes: &[u8], off: usize) -> u64 {
        u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap())
    }

    fn one_region_map() -> Vec<E820Entry> {
        let sys_start = 0x9_fc00;
        let sys_size: u64 = 0xe_0000 - 0x9_fc00;
        standard_e820(
            &[(0, 512 * MIB)],
            MIB,
            sys_start,
            sys_size,
            0xe000_0000,
            0x1000_0000,
        )
    }

    #[test]
    fn zero_page_setup_header_handling() {
        // no_header_sets_only_kernel_alignment: without a setup header only the
        // kernel alignment field is set.
        {
            let zp = ZeroPage::new(None).unwrap();
            let b = zp.as_bytes();
            assert_eq!(u32_at(b, 0x230), 0x0100_0000);
            assert!(b[0x1f1..0x230].iter().all(|&x| x == 0));
            assert!(b[0x234..0x290].iter().all(|&x| x == 0));
        }
        // setup_header_is_copied_to_0x1f1: the header bytes land at offset 0x1f1.
        {
            let hdr: Vec<u8> = (0..0x20u8).map(|i| i ^ 0x5a).collect();
            let zp = ZeroPage::new(Some(&hdr)).unwrap();
            assert_eq!(&zp.as_bytes()[0x1f1..0x1f1 + 0x20], &hdr[..]);
        }
        // too_long_setup_header_is_rejected: 0x9f bytes fit, 0xa0 bytes are rejected.
        {
            assert!(ZeroPage::new(Some(&vec![0u8; 0x9f])).is_ok());
            assert_eq!(
                ZeroPage::new(Some(&vec![0u8; 0xa0])).err(),
                Some(ZeroPageError::SetupHeaderTooLong(0xa0))
            );
        }
    }

    #[test]
    fn zero_page_set_fields() {
        // set_command_line_writes_fields_at_fixed_offsets: command line fields
        // and boot flags land at their fixed offsets.
        {
            let mut zp = ZeroPage::new(None).unwrap();
            zp.set_command_line(0x2000_0000, 0x7ff);
            let b = zp.as_bytes();
            assert_eq!(b[0x210], 0xff);
            assert_eq!(&b[0x1fe..0x200], &[0x55, 0xaa]);
            assert_eq!(u32_at(b, 0x202), 0x5372_6448);
            assert_eq!(u32_at(b, 0x228), 0x2000_0000);
            assert_eq!(u32_at(b, 0x238), 0x7ff);
        }
        // set_initrd_and_acpi_rsdp_write_at_fixed_offsets: initrd and ACPI RSDP
        // fields land at their fixed offsets.
        {
            let mut zp = ZeroPage::new(None).unwrap();
            zp.set_initrd(0x3000_0000, 0x12_3456);
            zp.set_acpi_rsdp(0x000e_0000_1234_5678);
            let b = zp.as_bytes();
            assert_eq!(u32_at(b, 0x218), 0x3000_0000);
            assert_eq!(u32_at(b, 0x21c), 0x12_3456);
            assert_eq!(u64_at(b, 0x070), 0x000e_0000_1234_5678);
        }
    }

    #[test]
    fn zero_page_add_e820() {
        // add_e820_writes_packed_entries_and_bumps_count: entries are packed
        // from offset 0x2d0 and the count at 0x1e8 is bumped per entry.
        {
            let mut zp = ZeroPage::new(None).unwrap();
            let a = E820Entry {
                addr: 0,
                size: 0x9_fc00,
                kind: E820_RAM,
            };
            let b = E820Entry {
                addr: 0x10_0000,
                size: 0x1f00_0000,
                kind: E820_RAM,
            };
            zp.add_e820(a).unwrap();
            zp.add_e820(b).unwrap();
            let bytes = zp.as_bytes();
            assert_eq!(bytes[0x1e8], 2);
            for (i, e) in [a, b].iter().enumerate() {
                let off = 0x2d0 + 20 * i;
                assert_eq!(u64_at(bytes, off), e.addr);
                assert_eq!(u64_at(bytes, off + 8), e.size);
                assert_eq!(u32_at(bytes, off + 16), e.kind);
            }
        }
        // e820_table_is_full_at_129th_entry: 128 entries fit, the 129th is
        // rejected with E820Full and the count stays at 128.
        {
            let mut zp = ZeroPage::new(None).unwrap();
            for i in 0..E820_MAX_ENTRIES as u64 {
                zp.add_e820(E820Entry {
                    addr: i * MIB,
                    size: MIB,
                    kind: E820_RAM,
                })
                .unwrap();
            }
            assert_eq!(zp.as_bytes()[0x1e8], 128);
            let extra = E820Entry {
                addr: 0,
                size: 0,
                kind: E820_RESERVED,
            };
            assert_eq!(zp.add_e820(extra), Err(ZeroPageError::E820Full));
            assert_eq!(zp.as_bytes()[0x1e8], 128);
            assert_eq!(zp.e820_entries().len(), 128);
        }
    }

    #[test]
    fn zero_page_standard_e820_map() {
        // standard_e820_single_region_has_four_entries_in_order: one usable region
        // yields four entries in address order.
        {
            let map = one_region_map();
            assert_eq!(map.len(), 4);
            assert_eq!(
                map[0],
                E820Entry {
                    addr: 0,
                    size: 0x9_fc00,
                    kind: E820_RAM
                }
            );
            assert_eq!(
                map[1],
                E820Entry {
                    addr: 0x9_fc00,
                    size: 0xe_0000 - 0x9_fc00,
                    kind: E820_RESERVED
                }
            );
            assert_eq!(
                map[2],
                E820Entry {
                    addr: 0xe000_0000,
                    size: 0x1000_0000,
                    kind: E820_RESERVED
                }
            );
            assert_eq!(
                map[3],
                E820Entry {
                    addr: MIB,
                    size: 512 * MIB - MIB,
                    kind: E820_RAM
                }
            );
        }
        // standard_e820_skips_region_entirely_below_himem: a region that ends below
        // the high-memory start contributes no RAM entry.
        {
            let map = standard_e820(
                &[(0, 0x8_0000)],
                MIB,
                0x9_fc00,
                0x4_0400,
                0xe000_0000,
                0x1000_0000,
            );
            assert_eq!(map.len(), 3);
            assert!(map.iter().all(|e| e.kind != E820_RAM || e.addr == 0));
        }
        // standard_e820_clips_region_straddling_himem: a region crossing the
        // high-memory start is clipped to begin at 1 MiB.
        {
            let map = standard_e820(
                &[(0x8_0000, 0x20_0000)],
                MIB,
                0x9_fc00,
                0x4_0400,
                0xe000_0000,
                0x1000_0000,
            );
            let last = map[map.len() - 1];
            assert_eq!(
                last,
                E820Entry {
                    addr: MIB,
                    size: 0x28_0000 - MIB,
                    kind: E820_RAM
                }
            );
        }
    }
}
