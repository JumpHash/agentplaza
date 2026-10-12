// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0

//! Pure x86_64 Linux kernel loader for the WHP backend.
//!
//! Loads an uncompressed ELF `vmlinux` or a `bzImage` into guest physical
//! memory, following the same rules as the KVM backend's
//! `arch::x86_64::load_kernel`:
//!
//! - ELF: every `PT_LOAD` segment is written at its physical address
//!   (`p_paddr`), file bytes first and then zeroes up to `p_memsz`. The entry
//!   point is `e_entry`.
//! - bzImage: the protected-mode payload (everything after the real-mode setup
//!   sectors) is written at `himem_start`. The entry point is
//!   `himem_start + 0x200`, the 64-bit entry. Only boot protocol 2.12 and newer
//!   with `XLF_KERNEL_64` is accepted.
//!
//! The image is untrusted input. Every offset and size read from it goes
//! through checked arithmetic and bounds-checked reads, so a malformed image
//! returns a [`KernelError`] and never panics or indexes out of range.

const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const EM_X86_64: u16 = 62;
const PT_LOAD: u32 = 1;
const ELF_HEADER_LEN: usize = 64;
const PHDR_LEN: u64 = 56;
const BZIMAGE_MIN_PROTOCOL: u16 = 0x020c;
const XLF_KERNEL_64: u16 = 1;
const BZIMAGE_ENTRY_OFFSET: u64 = 0x200;
/// Largest single write used to zero the bss part of a segment.
const ZERO_CHUNK: u64 = 64 * 1024;
static ZEROS: [u8; ZERO_CHUNK as usize] = [0; ZERO_CHUNK as usize];

/// Where loaded bytes go: guest physical memory.
pub trait GuestWriter {
    /// Writes `data` at guest physical address `gpa`; false if the range is not guest RAM.
    fn write_at(&mut self, gpa: u64, data: &[u8]) -> bool;
}

/// The kernel image format that was detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelFormat {
    /// An ELF64 `vmlinux`.
    Elf,
    /// A Linux `bzImage`.
    BzImage,
}

/// A kernel image after it has been loaded into guest memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedKernel {
    /// Guest physical address the vCPU starts executing at.
    pub entry: u64,
    /// The format of the image that was loaded.
    pub format: KernelFormat,
    /// bzImage only: image bytes [0x1f1, 0x202 + image[0x201]) — the setup header the zero page copies.
    pub setup_header: Option<Vec<u8>>,
    /// One past the highest guest address written.
    pub end: u64,
}

/// Why a kernel image could not be loaded.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum KernelError {
    /// The image is neither an ELF nor a bzImage.
    #[error("the kernel image is neither an ELF vmlinux nor a bzImage")]
    UnknownFormat,
    /// The ELF is not a little-endian x86_64 ELF64 image.
    #[error("the ELF image is not a little-endian x86_64 ELF64 executable")]
    UnsupportedElf,
    /// A structure the loader needs extends past the end of the image.
    #[error("the kernel image is truncated: {0}")]
    Truncated(&'static str),
    /// The bzImage boot protocol is older than 2.12, or it has no 64-bit entry.
    #[error("the bzImage boot protocol {0:#x} is older than 2.12 or has no 64-bit entry point")]
    NoBzImage64Entry(u16),
    /// A segment starts below `min` or does not fit in guest RAM.
    #[error("a kernel segment at {addr:#x} (+{len:#x}) lies below {min:#x} or outside guest memory")]
    SegmentOutOfRange {
        /// Guest physical address of the segment.
        addr: u64,
        /// Size of the segment in memory.
        len: u64,
        /// Lowest address the segment may use.
        min: u64,
    },
}

/// Loads `image` into `ram` and returns where the vCPU should start.
///
/// `himem_start` is the lowest guest address the kernel may occupy. Nothing is
/// written if the image fails validation before the writes begin.
pub fn load_kernel(
    image: &[u8],
    ram: &mut impl GuestWriter,
    himem_start: u64,
) -> Result<LoadedKernel, KernelError> {
    if image.starts_with(b"\x7fELF") {
        load_elf(image, ram, himem_start)
    } else if image.len() >= 0x206 && image.get(0x202..0x206) == Some(&b"HdrS"[..]) {
        load_bzimage(image, ram, himem_start)
    } else {
        Err(KernelError::UnknownFormat)
    }
}

/// Where an initrd of `initrd_size` goes: the top of the first RAM region
/// `[0, lowmem_size)`, aligned down to `page`.
///
/// Returns `None` if the initrd does not fit, or if `page` is zero or not a
/// power of two.
pub fn initrd_address(lowmem_size: u64, initrd_size: u64, page: u64) -> Option<u64> {
    if initrd_size > lowmem_size || page == 0 || !page.is_power_of_two() {
        return None;
    }
    Some((lowmem_size - initrd_size) & !(page - 1))
}

/// Reads `N` little-endian bytes at `off`, or `None` if they do not fit.
fn read_array<const N: usize>(bytes: &[u8], off: u64) -> Option<[u8; N]> {
    let start = usize::try_from(off).ok()?;
    let end = start.checked_add(N)?;
    bytes.get(start..end)?.try_into().ok()
}

fn read_u16(bytes: &[u8], off: u64) -> Option<u16> {
    read_array(bytes, off).map(u16::from_le_bytes)
}

fn read_u32(bytes: &[u8], off: u64) -> Option<u32> {
    read_array(bytes, off).map(u32::from_le_bytes)
}

fn read_u64(bytes: &[u8], off: u64) -> Option<u64> {
    read_array(bytes, off).map(u64::from_le_bytes)
}

/// The `len` bytes at `off`, or `None` if they do not fit.
fn sub_slice(bytes: &[u8], off: u64, len: u64) -> Option<&[u8]> {
    let start = usize::try_from(off).ok()?;
    let len = usize::try_from(len).ok()?;
    bytes.get(start..start.checked_add(len)?)
}

fn segment_error(addr: u64, len: u64, min: u64) -> KernelError {
    KernelError::SegmentOutOfRange { addr, len, min }
}

/// One validated `PT_LOAD` segment, ready to be written.
struct Segment<'a> {
    /// Guest physical address of the first file byte.
    paddr: u64,
    /// `p_memsz`, kept for error reporting.
    memsz: u64,
    /// The file bytes, written at `paddr`.
    data: &'a [u8],
    /// Guest address where zeroing starts (`paddr + p_filesz`).
    zero_at: u64,
    /// Number of zero bytes after the file bytes (`p_memsz - p_filesz`).
    zero_len: u64,
}

fn load_elf(
    image: &[u8],
    ram: &mut impl GuestWriter,
    himem_start: u64,
) -> Result<LoadedKernel, KernelError> {
    if image.len() < ELF_HEADER_LEN {
        return Err(KernelError::Truncated("ELF header"));
    }
    if image.get(4) != Some(&ELFCLASS64)
        || image.get(5) != Some(&ELFDATA2LSB)
        || read_u16(image, 0x12) != Some(EM_X86_64)
    {
        return Err(KernelError::UnsupportedElf);
    }
    let entry = read_u64(image, 0x18).ok_or(KernelError::Truncated("ELF header"))?;
    let phoff = read_u64(image, 0x20).ok_or(KernelError::Truncated("ELF header"))?;
    let phentsize = read_u16(image, 0x36).ok_or(KernelError::Truncated("ELF header"))?;
    let phnum = read_u16(image, 0x38).ok_or(KernelError::Truncated("ELF header"))?;

    // Validate every segment before writing any of them.
    let mut segments = Vec::new();
    let mut max_end = 0u64;
    if phnum > 0 {
        if u64::from(phentsize) < PHDR_LEN {
            return Err(KernelError::Truncated(
                "program header entry is smaller than 56 bytes",
            ));
        }
        let table_len = u64::from(phnum) * u64::from(phentsize);
        let table = sub_slice(image, phoff, table_len)
            .ok_or(KernelError::Truncated("program header table"))?;
        for i in 0..u64::from(phnum) {
            let ph = sub_slice(table, i * u64::from(phentsize), PHDR_LEN)
                .ok_or(KernelError::Truncated("program header"))?;
            if read_u32(ph, 0) != Some(PT_LOAD) {
                continue;
            }
            let trunc_ph = || KernelError::Truncated("PT_LOAD header");
            let p_offset = read_u64(ph, 8).ok_or_else(trunc_ph)?;
            let p_paddr = read_u64(ph, 0x18).ok_or_else(trunc_ph)?;
            let p_filesz = read_u64(ph, 0x20).ok_or_else(trunc_ph)?;
            let p_memsz = read_u64(ph, 0x28).ok_or_else(trunc_ph)?;

            if p_paddr < himem_start {
                return Err(segment_error(p_paddr, p_memsz, himem_start));
            }
            let zero_len = p_memsz
                .checked_sub(p_filesz)
                .ok_or(KernelError::Truncated("PT_LOAD file size exceeds memory size"))?;
            let data = sub_slice(image, p_offset, p_filesz)
                .ok_or(KernelError::Truncated("PT_LOAD file data"))?;
            let zero_at = p_paddr
                .checked_add(p_filesz)
                .ok_or_else(|| segment_error(p_paddr, p_memsz, himem_start))?;
            let end = p_paddr
                .checked_add(p_memsz)
                .ok_or_else(|| segment_error(p_paddr, p_memsz, himem_start))?;
            max_end = max_end.max(end);
            segments.push(Segment {
                paddr: p_paddr,
                memsz: p_memsz,
                data,
                zero_at,
                zero_len,
            });
        }
    }

    for seg in &segments {
        if !ram.write_at(seg.paddr, seg.data) {
            return Err(segment_error(seg.paddr, seg.memsz, himem_start));
        }
        let mut gpa = seg.zero_at;
        let mut remaining = seg.zero_len;
        while remaining > 0 {
            let n = remaining.min(ZERO_CHUNK);
            // `n` is at most ZERO_CHUNK, which is the length of ZEROS.
            if !ram.write_at(gpa, &ZEROS[..n as usize]) {
                return Err(segment_error(seg.paddr, seg.memsz, himem_start));
            }
            gpa = gpa
                .checked_add(n)
                .ok_or_else(|| segment_error(seg.paddr, seg.memsz, himem_start))?;
            remaining -= n;
        }
    }

    Ok(LoadedKernel {
        entry,
        format: KernelFormat::Elf,
        setup_header: None,
        end: max_end,
    })
}

fn load_bzimage(
    image: &[u8],
    ram: &mut impl GuestWriter,
    himem_start: u64,
) -> Result<LoadedKernel, KernelError> {
    let version = read_u16(image, 0x206).ok_or(KernelError::Truncated("bzImage header"))?;
    if version < BZIMAGE_MIN_PROTOCOL {
        return Err(KernelError::NoBzImage64Entry(version));
    }
    let xloadflags = read_u16(image, 0x236).ok_or(KernelError::Truncated("bzImage header"))?;
    if xloadflags & XLF_KERNEL_64 == 0 {
        return Err(KernelError::NoBzImage64Entry(version));
    }

    let raw_sects = *image
        .get(0x1f1)
        .ok_or(KernelError::Truncated("bzImage setup_sects"))?;
    let setup_sects = if raw_sects == 0 {
        4
    } else {
        u64::from(raw_sects)
    };
    let payload_off = (setup_sects + 1) * 512;
    let payload = usize::try_from(payload_off)
        .ok()
        .and_then(|off| image.get(off..))
        .ok_or(KernelError::Truncated("bzImage payload"))?;

    let jump = *image
        .get(0x201)
        .ok_or(KernelError::Truncated("bzImage header"))?;
    let setup_header = image
        .get(0x1f1..0x202 + usize::from(jump))
        .ok_or(KernelError::Truncated("bzImage setup header"))?
        .to_vec();

    let payload_len = payload.len() as u64;
    let write_err = || segment_error(himem_start, payload_len, himem_start);
    let entry = himem_start
        .checked_add(BZIMAGE_ENTRY_OFFSET)
        .ok_or_else(write_err)?;
    let end = himem_start.checked_add(payload_len).ok_or_else(write_err)?;
    if !ram.write_at(himem_start, payload) {
        return Err(write_err());
    }

    Ok(LoadedKernel {
        entry,
        format: KernelFormat::BzImage,
        setup_header: Some(setup_header),
        end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HIMEM: u64 = 0x10_0000;

    /// Guest RAM at gpa 0. Filled with 0xAA so tests can tell zero-fill from untouched memory.
    struct TestRam {
        mem: Vec<u8>,
    }

    impl TestRam {
        fn new() -> Self {
            Self {
                mem: vec![0xAA; 32 << 20],
            }
        }

        fn bytes(&self, gpa: u64, len: usize) -> &[u8] {
            let start = gpa as usize;
            &self.mem[start..start + len]
        }
    }

    impl GuestWriter for TestRam {
        fn write_at(&mut self, gpa: u64, data: &[u8]) -> bool {
            let Ok(start) = usize::try_from(gpa) else {
                return false;
            };
            let Some(end) = start.checked_add(data.len()) else {
                return false;
            };
            match self.mem.get_mut(start..end) {
                Some(dst) => {
                    dst.copy_from_slice(data);
                    true
                }
                None => false,
            }
        }
    }

    /// A minimal ELF64 image: header, one PT_LOAD program header, then `payload`.
    fn elf_image(machine: u16, entry: u64, paddr: u64, memsz: u64, payload: &[u8]) -> Vec<u8> {
        let mut img = vec![0u8; 64];
        img[..4].copy_from_slice(b"\x7fELF");
        img[4] = 2; // ELFCLASS64
        img[5] = 1; // ELFDATA2LSB
        img[6] = 1; // EV_CURRENT
        img[0x12..0x14].copy_from_slice(&machine.to_le_bytes());
        img[0x18..0x20].copy_from_slice(&entry.to_le_bytes());
        img[0x20..0x28].copy_from_slice(&64u64.to_le_bytes()); // e_phoff
        img[0x36..0x38].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize
        img[0x38..0x3a].copy_from_slice(&1u16.to_le_bytes()); // e_phnum

        let data_off = 64u64 + 56;
        let mut ph = vec![0u8; 56];
        ph[0..4].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
        ph[8..16].copy_from_slice(&data_off.to_le_bytes()); // p_offset
        ph[0x18..0x20].copy_from_slice(&paddr.to_le_bytes()); // p_paddr
        ph[0x20..0x28].copy_from_slice(&(payload.len() as u64).to_le_bytes()); // p_filesz
        ph[0x28..0x30].copy_from_slice(&memsz.to_le_bytes()); // p_memsz
        img.extend_from_slice(&ph);
        img.extend_from_slice(payload);
        img
    }

    /// A minimal bzImage: setup area of `(setup_sects + 1) * 512` bytes (0 means 4), then `payload`.
    fn bzimage(setup_sects: u8, version: u16, xloadflags: u16, payload: &[u8]) -> Vec<u8> {
        let sects = if setup_sects == 0 {
            4
        } else {
            usize::from(setup_sects)
        };
        let mut img = vec![0u8; (sects + 1) * 512];
        img[0x1f1] = setup_sects;
        img[0x201] = 0x10; // jump offset: setup header is 0x1f1..0x212
        img[0x202..0x206].copy_from_slice(b"HdrS");
        img[0x206..0x208].copy_from_slice(&version.to_le_bytes());
        img[0x236..0x238].copy_from_slice(&xloadflags.to_le_bytes());
        img.extend_from_slice(payload);
        img
    }

    #[test]
    fn elf_loads_file_bytes_and_zero_fills_bss() {
        let mut ram = TestRam::new();
        let image = elf_image(62, 0x20_0010, 0x20_0000, 0x300, &[0x11; 0x100]);
        let loaded = load_kernel(&image, &mut ram, HIMEM).unwrap();
        assert_eq!(loaded.entry, 0x20_0010);
        assert_eq!(loaded.format, KernelFormat::Elf);
        assert_eq!(loaded.setup_header, None);
        assert_eq!(loaded.end, 0x20_0300);
        assert!(ram.bytes(0x20_0000, 0x100).iter().all(|&b| b == 0x11));
        assert!(ram.bytes(0x20_0100, 0x200).iter().all(|&b| b == 0));
        assert_eq!(ram.bytes(0x20_0300, 1)[0], 0xAA);
    }

    #[test]
    fn large_bss_is_zeroed_across_chunks() {
        let mut ram = TestRam::new();
        let memsz = 2 * (64 * 1024) + 0x1234;
        let image = elf_image(62, 0x20_0000, 0x20_0000, memsz, &[]);
        let loaded = load_kernel(&image, &mut ram, HIMEM).unwrap();
        assert_eq!(loaded.end, 0x20_0000 + memsz);
        assert!(ram
            .bytes(0x20_0000, memsz as usize)
            .iter()
            .all(|&b| b == 0));
        assert_eq!(ram.bytes(0x20_0000 + memsz, 1)[0], 0xAA);
    }

    #[test]
    fn elf_segment_below_himem_is_rejected() {
        let mut ram = TestRam::new();
        let image = elf_image(62, 0x1000, 0x1000, 0x100, &[1; 16]);
        assert_eq!(
            load_kernel(&image, &mut ram, HIMEM),
            Err(KernelError::SegmentOutOfRange {
                addr: 0x1000,
                len: 0x100,
                min: HIMEM,
            })
        );
    }

    #[test]
    fn elf_segment_outside_ram_is_rejected() {
        let mut ram = TestRam::new();
        let image = elf_image(62, 0x3000_0000, 0x3000_0000, 0x100, &[1; 16]);
        assert!(matches!(
            load_kernel(&image, &mut ram, HIMEM),
            Err(KernelError::SegmentOutOfRange { .. })
        ));
    }

    #[test]
    fn wrong_machine_is_unsupported() {
        let mut ram = TestRam::new();
        let image = elf_image(40, 0x20_0000, 0x20_0000, 0x10, &[1; 4]); // EM_ARM
        assert_eq!(
            load_kernel(&image, &mut ram, HIMEM),
            Err(KernelError::UnsupportedElf)
        );
    }

    #[test]
    fn truncated_program_header_table_is_rejected() {
        let mut ram = TestRam::new();
        let mut image = elf_image(62, HIMEM, HIMEM, 0x10, &[1; 4]);
        image[0x38..0x3a].copy_from_slice(&2u16.to_le_bytes()); // claim two headers
        image.truncate(64 + 56); // only one is present
        assert!(matches!(
            load_kernel(&image, &mut ram, HIMEM),
            Err(KernelError::Truncated(_))
        ));
    }

    #[test]
    fn overflowing_or_huge_segment_fields_do_not_panic() {
        let mut ram = TestRam::new();

        // p_paddr + p_memsz overflows.
        let mut image = elf_image(62, HIMEM, HIMEM, 0x10, &[1; 4]);
        image[64 + 0x18..64 + 0x20].copy_from_slice(&(u64::MAX - 0x10).to_le_bytes());
        image[64 + 0x20..64 + 0x28].copy_from_slice(&0u64.to_le_bytes()); // p_filesz
        image[64 + 0x28..64 + 0x30].copy_from_slice(&0x20u64.to_le_bytes()); // p_memsz
        assert!(matches!(
            load_kernel(&image, &mut ram, HIMEM),
            Err(KernelError::SegmentOutOfRange { .. })
        ));

        // p_offset points past the end of the image.
        let mut image = elf_image(62, HIMEM, HIMEM, 0x10, &[1; 4]);
        image[64 + 8..64 + 16].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(matches!(
            load_kernel(&image, &mut ram, HIMEM),
            Err(KernelError::Truncated(_))
        ));
    }

    #[test]
    fn bzimage_loads_payload_at_himem_and_returns_setup_header() {
        let mut ram = TestRam::new();
        let image = bzimage(1, 0x020f, 1, &[0x77; 0x300]);
        let loaded = load_kernel(&image, &mut ram, HIMEM).unwrap();
        assert_eq!(loaded.entry, HIMEM + 0x200);
        assert_eq!(loaded.format, KernelFormat::BzImage);
        assert_eq!(
            loaded.setup_header.as_deref(),
            Some(&image[0x1f1..0x212])
        );
        assert_eq!(loaded.end, HIMEM + 0x300);
        assert!(ram.bytes(HIMEM, 0x300).iter().all(|&b| b == 0x77));
    }

    #[test]
    fn bzimage_setup_sects_zero_means_four() {
        let mut ram = TestRam::new();
        // With 4 setup sectors the payload starts at 5 * 512 = 2560.
        let image = bzimage(0, 0x020c, 1, &[0x55; 0x100]);
        let loaded = load_kernel(&image, &mut ram, HIMEM).unwrap();
        assert_eq!(loaded.end, HIMEM + 0x100);
        assert!(ram.bytes(HIMEM, 0x100).iter().all(|&b| b == 0x55));
    }

    #[test]
    fn bzimage_old_protocol_or_no_64bit_entry_is_rejected() {
        let mut ram = TestRam::new();
        assert_eq!(
            load_kernel(&bzimage(1, 0x020b, 1, &[0; 16]), &mut ram, HIMEM),
            Err(KernelError::NoBzImage64Entry(0x020b))
        );
        assert_eq!(
            load_kernel(&bzimage(1, 0x020c, 0, &[0; 16]), &mut ram, HIMEM),
            Err(KernelError::NoBzImage64Entry(0x020c))
        );
    }

    #[test]
    fn unknown_formats_are_rejected() {
        let mut ram = TestRam::new();
        assert_eq!(
            load_kernel(&[0x42; 4096], &mut ram, HIMEM),
            Err(KernelError::UnknownFormat)
        );
        assert_eq!(
            load_kernel(&[], &mut ram, HIMEM),
            Err(KernelError::UnknownFormat)
        );
    }

    #[test]
    fn tiny_and_empty_images_never_panic() {
        let mut ram = TestRam::new();
        for len in 0..0x260usize {
            let mut elf = vec![0xffu8; len];
            if len >= 4 {
                elf[..4].copy_from_slice(b"\x7fELF");
            }
            let _ = load_kernel(&elf, &mut ram, HIMEM);

            let mut bz = vec![0xffu8; len];
            if len >= 0x206 {
                bz[0x202..0x206].copy_from_slice(b"HdrS");
            }
            let _ = load_kernel(&bz, &mut ram, HIMEM);
        }
    }

    #[test]
    fn corrupting_any_header_byte_never_panics() {
        let mut ram = TestRam::new();
        let base_elf = elf_image(62, HIMEM, HIMEM, 0x100, &[1; 0x40]);
        let base_bz = bzimage(1, 0x020f, 1, &[2; 0x40]);
        for base in [base_elf, base_bz] {
            for i in 0..base.len() {
                for v in [0x00u8, 0x7f, 0xff] {
                    let mut img = base.clone();
                    img[i] = v;
                    let _ = load_kernel(&img, &mut ram, HIMEM);
                }
            }
        }
    }

    #[test]
    fn initrd_goes_to_top_of_lowmem_aligned_down() {
        assert_eq!(
            initrd_address(0x1000_0000, 0x12_3456, 0x1000),
            Some(0x0FED_C000)
        );
        assert_eq!(initrd_address(0x1000_0000, 0x12_3456, 1), Some(0x0FED_CBAA));
        assert_eq!(initrd_address(0x1000_0000, 0x1000_0000, 0x1000), Some(0));
        assert_eq!(initrd_address(0x1000_0000, 0x1000_0001, 0x1000), None);
        assert_eq!(initrd_address(0x1000_0000, 0x10, 0), None);
        assert_eq!(initrd_address(0x1000_0000, 0x10, 3), None);
    }
}
