// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port. The page selection follows CubeSandbox's
// hypervisor/vmm/src/pagemap_anon.rs (Apache-2.0, Copyright Tencent): a page is
// saved when /proc/self/pagemap says it is present or swapped, and, for a diff
// of a guest restored from a file, only when it is anonymous (not PM_FILE) —
// the pages the guest wrote since its restore.

//! Guest memory in snapshots.
//!
//! **Writing.** [`write_memory`] walks `/proc/self/pagemap` for each guest region and writes only
//! the pages that hold something: a page the guest never touched is neither present nor swapped,
//! and stays a hole in the (sparse) memory file. A [`SnapshotKind::Diff`] writes only the
//! *anonymous* pages — for a guest whose memory is a private mapping of an earlier memory file
//! ([`crate::vstate::memory::map_private`]), those are exactly the pages it changed since, and the
//! caller has already put a reflinked copy of that earlier file where the diff is written
//! ([`crate::clonefile::clone_file`]), so the result is a complete memory file at the cost of the
//! changed pages.
//!
//! **Layout.** [`layout`] places the regions back to back in guest-address order, each at a
//! page-aligned offset.

use std::fs::File;
use std::os::unix::fs::FileExt;

use vm_memory::GuestMemoryBackend;

use crate::spawner::SnapshotKind;
use crate::vstate::memory::{
    Address, Bytes, GuestMemoryMmap, GuestMemoryRegion, GuestRegionMmapExt, MemoryRegionAddress,
    host_page_size, u64_to_usize, usize_to_u64,
};
use crate::vstate::snapshot::{RegionMeta, SnapshotError};

/// The kernel's per-page state, one 8-byte entry per host page of this process's address space.
const PAGEMAP: &str = "/proc/self/pagemap";

/// This process's pagemap, opened before the process entered its jail (a jailed VMM's new root has no
/// `/proc`). The descriptor keeps naming this process after `pivot_root`.
static EARLY_PAGEMAP: std::sync::OnceLock<File> = std::sync::OnceLock::new();

/// Opens this process's pagemap now, for snapshots taken after the process is jailed (see
/// [`EARLY_PAGEMAP`]). The builder calls it before it enters the jail; a failure is left for the
/// snapshot to report.
pub fn open_pagemap_early() {
    if EARLY_PAGEMAP.get().is_none() {
        if let Ok(file) = File::open(PAGEMAP) {
            let _ = EARLY_PAGEMAP.set(file);
        }
    }
}
/// Bytes in one pagemap entry.
const PAGEMAP_ENTRY_SIZE: u64 = 8;
/// Bit 63: the page is present in RAM.
const PAGEMAP_PRESENT: u64 = 1 << 63;
/// Bit 62: the page is in swap.
const PAGEMAP_SWAPPED: u64 = 1 << 62;
/// Bit 61 (`PM_FILE`): the page is file-backed or shared. A page the guest wrote through a private
/// file mapping is copy-on-write anonymous, and has it clear. The flag is readable without
/// privileges; the page frame numbers are not used at all.
const PAGEMAP_FILE: u64 = 1 << 61;
/// Pagemap entries read per `read_exact_at`: 4 KiB of entries.
const PAGEMAP_BATCH_PAGES: u64 = 512;
/// The most bytes of guest memory written at once, so a run of a whole region streams through a
/// buffer of this size rather than being copied whole.
const MAX_WRITE_BYTES: u64 = 1 << 20;
/// How the memory file is named in errors: the writer only has the open file, not its path.
const MEMORY_FILE: &str = "guest memory file";

/// The memory file layout of `mem`: one [`RegionMeta`] per region, in guest-address order.
pub fn layout(mem: &GuestMemoryMmap) -> Vec<RegionMeta> {
    let page = usize_to_u64(host_page_size());
    let mut regions: Vec<(u64, u64)> = mem
        .iter()
        .map(|region| (region.start_addr().raw_value(), region.len()))
        .collect();
    regions.sort_unstable();
    let mut offset = 0;
    regions
        .into_iter()
        .map(|(guest_addr, len)| {
            let meta = RegionMeta {
                guest_addr,
                len,
                offset,
            };
            offset += len.div_ceil(page) * page;
            meta
        })
        .collect()
}

/// Writes the guest memory of `mem` into `file` at the offsets of `regions` (from [`layout`]),
/// setting the file's length to cover the whole layout. Returns the bytes of memory written.
/// Called with the VM paused and its devices locked, so nothing writes guest memory meanwhile.
pub fn write_memory(
    mem: &GuestMemoryMmap,
    regions: &[RegionMeta],
    file: &File,
    kind: SnapshotKind,
) -> Result<u64, SnapshotError> {
    let page = usize_to_u64(host_page_size());
    let in_memory = mem.iter().count();
    if regions.len() != in_memory {
        return Err(SnapshotError::Mismatch(format!(
            "the layout has {} regions and the memory has {in_memory}",
            regions.len()
        )));
    }
    let opened;
    let pagemap = match EARLY_PAGEMAP.get() {
        Some(file) => file,
        None => {
            opened = File::open(PAGEMAP).map_err(|source| io_error(PAGEMAP, source))?;
            &opened
        }
    };
    // A full snapshot's file holds the touched pages and holes everywhere else, so content left in
    // a reused file from an earlier snapshot must not survive. A diff keeps its base copy.
    if matches!(kind, SnapshotKind::Full) {
        file.set_len(0)
            .map_err(|source| io_error(MEMORY_FILE, source))?;
    }
    let mut written = 0;
    let mut end = 0;
    let mut buf = Vec::new();
    for meta in regions {
        let region = mem
            .iter()
            .find(|r| r.start_addr().raw_value() == meta.guest_addr && r.len() == meta.len)
            .ok_or_else(|| {
                SnapshotError::Mismatch(format!(
                    "no guest memory region at {:#x} of {} bytes",
                    meta.guest_addr, meta.len
                ))
            })?;
        if meta.len % page != 0 || meta.offset % page != 0 {
            return Err(SnapshotError::Mismatch(format!(
                "region at {:#x} is not page aligned",
                meta.guest_addr
            )));
        }
        written += write_region(region, meta, &pagemap, file, kind, &mut buf)?;
        end = end.max(meta.offset + meta.len);
    }
    file.set_len(end)
        .map_err(|source| io_error(MEMORY_FILE, source))?;
    Ok(written)
}

/// Writes the selected pages of one region: the pagemap is read in batches, and each run of
/// adjacent selected pages is written in one go.
fn write_region(
    region: &GuestRegionMmapExt,
    meta: &RegionMeta,
    pagemap: &File,
    file: &File,
    kind: SnapshotKind,
    buf: &mut Vec<u8>,
) -> Result<u64, SnapshotError> {
    let page = usize_to_u64(host_page_size());
    let host = region
        .get_host_address(MemoryRegionAddress(0))
        .map_err(|e| SnapshotError::State(format!("guest memory has no host address: {e}")))?;
    let host = usize_to_u64(host as usize);
    if host % page != 0 {
        return Err(SnapshotError::State(format!(
            "guest memory at host address {host:#x} is not page aligned"
        )));
    }
    let first_page = host / page;
    let pages = meta.len / page;

    let mut written = 0;
    // The pending run of selected pages, as (first page, page count) within the region.
    let mut run: Option<(u64, u64)> = None;
    let mut entries = vec![0u8; u64_to_usize(PAGEMAP_BATCH_PAGES * PAGEMAP_ENTRY_SIZE)];
    let mut done = 0;
    while done < pages {
        let batch = (pages - done).min(PAGEMAP_BATCH_PAGES);
        let bytes = u64_to_usize(batch * PAGEMAP_ENTRY_SIZE);
        pagemap
            .read_exact_at(
                &mut entries[..bytes],
                (first_page + done) * PAGEMAP_ENTRY_SIZE,
            )
            .map_err(|source| io_error(PAGEMAP, source))?;
        for (i, chunk) in entries[..bytes].chunks_exact(8).enumerate() {
            let page_index = done + usize_to_u64(i);
            if selected(pagemap_entry(chunk), kind) {
                match run {
                    Some((start, count)) if start + count == page_index => {
                        run = Some((start, count + 1));
                    }
                    _ => {
                        written += flush_run(&mut run, region, meta, file, buf)?;
                        run = Some((page_index, 1));
                    }
                }
            } else {
                written += flush_run(&mut run, region, meta, file, buf)?;
            }
        }
        done += batch;
    }
    written += flush_run(&mut run, region, meta, file, buf)?;
    Ok(written)
}

/// Writes the pending run of pages, if there is one, and returns the bytes written.
fn flush_run(
    run: &mut Option<(u64, u64)>,
    region: &GuestRegionMmapExt,
    meta: &RegionMeta,
    file: &File,
    buf: &mut Vec<u8>,
) -> Result<u64, SnapshotError> {
    match run.take() {
        Some((start, count)) => write_pages(region, meta, file, start, count, buf),
        None => Ok(0),
    }
}

/// Writes `count` pages of `region` from its page `start` to the memory file, at the region's
/// offset in the file, in writes of at most [`MAX_WRITE_BYTES`].
fn write_pages(
    region: &GuestRegionMmapExt,
    meta: &RegionMeta,
    file: &File,
    start: u64,
    count: u64,
    buf: &mut Vec<u8>,
) -> Result<u64, SnapshotError> {
    let page = usize_to_u64(host_page_size());
    let max_pages = (MAX_WRITE_BYTES / page).max(1);
    let mut done = 0;
    while done < count {
        let n = (count - done).min(max_pages);
        let guest_offset = (start + done) * page;
        buf.resize(u64_to_usize(n * page), 0);
        region
            .read_slice(buf.as_mut_slice(), MemoryRegionAddress(guest_offset))
            .map_err(|e| SnapshotError::State(format!("cannot read guest memory: {e}")))?;
        file.write_all_at(buf.as_slice(), meta.offset + guest_offset)
            .map_err(|source| io_error(MEMORY_FILE, source))?;
        done += n;
    }
    Ok(count * page)
}

/// Whether a page with this pagemap entry goes into a snapshot of `kind`.
fn selected(entry: u64, kind: SnapshotKind) -> bool {
    let present = entry & PAGEMAP_PRESENT != 0;
    let swapped = entry & PAGEMAP_SWAPPED != 0;
    let file_backed = entry & PAGEMAP_FILE != 0;
    match kind {
        // Everything the guest has touched: present pages, and the ones swapped out.
        SnapshotKind::Full => present || swapped,
        // A swapped page is always saved. A present page is saved only when it is anonymous: a
        // file page is still the base file's content, which the diff's reflinked copy already holds.
        SnapshotKind::Diff => swapped || (present && !file_backed),
    }
}

/// Decodes one pagemap entry from its 8 native-endian bytes.
fn pagemap_entry(chunk: &[u8]) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(chunk);
    u64::from_ne_bytes(bytes)
}

/// Wraps an I/O error on `path` as a snapshot error.
fn io_error(path: &str, source: std::io::Error) -> SnapshotError {
    SnapshotError::Io {
        path: path.to_string(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use vm_memory::GuestRegionCollection;

    use super::*;
    use crate::vstate::memory::{GuestAddress, MemoryError, guest_memory_from_ranges, map_private};

    /// A scratch directory unique to this test run.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("apz-snapshot-memory-{}-{name}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A file for a memory image, created empty for writing.
    fn create(path: &PathBuf) -> File {
        File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .unwrap()
    }

    /// Guest memory of one region of `pages` host pages at guest address 0.
    fn memory(pages: usize) -> GuestMemoryMmap {
        let page = host_page_size();
        guest_memory_from_ranges(&[(GuestAddress(0), pages * page)]).unwrap()
    }

    /// Guest memory of `mem`'s layout, mapped privately from `file`.
    fn mapped(file: &File, regions: &[RegionMeta]) -> GuestMemoryMmap {
        let maps = map_private(file, regions).unwrap();
        GuestRegionCollection::from_regions(
            maps.into_iter()
                .map(|region| GuestRegionMmapExt::dram_from_mmap_region(region, 0))
                .collect(),
        )
        .unwrap()
    }

    /// Fills host page `index` of guest memory `mem` with `byte`.
    fn fill_page(mem: &GuestMemoryMmap, index: usize, byte: u8) {
        let page = host_page_size();
        mem.write_slice(&vec![byte; page], GuestAddress((index * page) as u64))
            .unwrap();
    }

    /// The bytes of host page `index` of `file`'s contents.
    fn page_of(contents: &[u8], index: usize) -> &[u8] {
        let page = host_page_size();
        &contents[index * page..(index + 1) * page]
    }

    #[test]
    fn layout_is_in_guest_order_at_page_aligned_offsets() {
        let page = host_page_size();
        let mem = guest_memory_from_ranges(&[
            (GuestAddress(0), 3 * page),
            (GuestAddress(0x20_0000), 2 * page),
        ])
        .unwrap();
        let p = usize_to_u64(page);
        assert_eq!(
            layout(&mem),
            vec![
                RegionMeta {
                    guest_addr: 0,
                    len: 3 * p,
                    offset: 0,
                },
                RegionMeta {
                    guest_addr: 0x20_0000,
                    len: 2 * p,
                    offset: 3 * p,
                },
            ]
        );
    }

    #[test]
    fn full_writes_the_touched_pages_and_diff_of_anonymous_memory_matches() {
        let mem = memory(8);
        let page = host_page_size();
        // Pages 2 and 3 are one run, page 5 another; the rest are never touched.
        for index in [2, 3, 5] {
            fill_page(&mem, index, 0xAB);
        }
        let regions = layout(&mem);
        let dir = scratch("full");

        let full = dir.join("full");
        let written = write_memory(&mem, &regions, &create(&full), SnapshotKind::Full).unwrap();
        assert_eq!(written, usize_to_u64(3 * page));
        let contents = fs::read(&full).unwrap();
        assert_eq!(contents.len(), 8 * page);
        for index in 0..8 {
            let expected = if [2, 3, 5].contains(&index) { 0xAB } else { 0 };
            assert!(
                page_of(&contents, index).iter().all(|&b| b == expected),
                "page {index}"
            );
        }

        // Booted (anonymous) memory: a diff saves the same pages as a full snapshot.
        let diff = dir.join("diff");
        let written = write_memory(&mem, &regions, &create(&diff), SnapshotKind::Diff).unwrap();
        assert_eq!(written, usize_to_u64(3 * page));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn private_mapping_reads_the_file_and_writes_stay_in_memory() {
        let source = memory(4);
        let page = host_page_size();
        fill_page(&source, 1, 0x11);
        let regions = layout(&source);
        let dir = scratch("private");
        let path = dir.join("memory");
        write_memory(&source, &regions, &create(&path), SnapshotKind::Full).unwrap();
        drop(source);

        let restored = mapped(&File::open(&path).unwrap(), &regions);
        let mut read = vec![0u8; page];
        restored
            .read_slice(&mut read, GuestAddress(page as u64))
            .unwrap();
        assert!(read.iter().all(|&b| b == 0x11));
        restored.read_slice(&mut read, GuestAddress(0)).unwrap();
        assert!(read.iter().all(|&b| b == 0));

        // A write through the mapping is copy-on-write: the file keeps its content.
        fill_page(&restored, 1, 0x22);
        let contents = fs::read(&path).unwrap();
        assert!(page_of(&contents, 1).iter().all(|&b| b == 0x11));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn diff_writes_only_the_pages_the_guest_wrote_since_restore() {
        let source = memory(8);
        let page = host_page_size();
        fill_page(&source, 2, 0x11);
        fill_page(&source, 5, 0x11);
        let regions = layout(&source);
        let dir = scratch("diff");
        let path = dir.join("memory");
        write_memory(&source, &regions, &create(&path), SnapshotKind::Full).unwrap();
        drop(source);

        let restored = mapped(&File::open(&path).unwrap(), &regions);
        // Reading pages 2 and 5 leaves them file-backed: present, but not written.
        let mut read = vec![0u8; page];
        restored
            .read_slice(&mut read, GuestAddress((2 * page) as u64))
            .unwrap();
        restored
            .read_slice(&mut read, GuestAddress((5 * page) as u64))
            .unwrap();
        // Writing page 2 makes it anonymous: the only page the diff holds.
        fill_page(&restored, 2, 0xCD);

        let diff = dir.join("diff");
        let written =
            write_memory(&restored, &regions, &create(&diff), SnapshotKind::Diff).unwrap();
        assert_eq!(written, usize_to_u64(page));
        let contents = fs::read(&diff).unwrap();
        assert!(page_of(&contents, 2).iter().all(|&b| b == 0xCD));
        assert!(
            contents
                .iter()
                .enumerate()
                .all(|(i, &b)| { (2 * page..3 * page).contains(&i) || b == 0 })
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn map_private_refuses_a_short_file_and_unaligned_regions() {
        let page = usize_to_u64(host_page_size());
        let dir = scratch("refuse");
        let path = dir.join("memory");
        fs::write(&path, vec![0u8; 2 * host_page_size()]).unwrap();
        let file = File::open(&path).unwrap();

        let short = [RegionMeta {
            guest_addr: 0,
            len: 3 * page,
            offset: 0,
        }];
        assert!(matches!(
            map_private(&file, &short),
            Err(MemoryError::FileTooShort { .. })
        ));
        let unaligned = [RegionMeta {
            guest_addr: 0,
            len: page,
            offset: 1,
        }];
        assert!(matches!(
            map_private(&file, &unaligned),
            Err(MemoryError::UnalignedFileRegion { .. })
        ));
        fs::remove_dir_all(&dir).unwrap();
    }
}
