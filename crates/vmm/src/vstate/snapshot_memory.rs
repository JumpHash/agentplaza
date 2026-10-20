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
//! **Capturing.** [`capture_memory`] makes the same selection but copies the pages into RAM, so the
//! VM can resume before they are written; [`write_captured`] writes them to the file afterwards.
//!
//! **Layout.** [`layout`] places the regions back to back in guest-address order, each at a
//! page-aligned offset.
//!
//! **Working set.** Before a snapshot writes anything, the kernel is asked which pages of each region
//! are resident ([`crate::sys::resident_pages`]), and [`resident_runs`] turns them into runs of
//! memory-file offsets: the snapshot's working-set hint ([`crate::vstate::snapshot::WORKING_SET_FILE`]).
//! A restore reads the hint ahead ([`read_ahead`]), so the guest's first touches of those pages come
//! from the page cache rather than the disk.

use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::Path;

use vm_memory::GuestMemoryBackend;

use crate::spawner::SnapshotKind;
use crate::vstate::memory::{
    Address, Bytes, GuestMemoryMmap, GuestMemoryRegion, GuestRegionMmapExt, MemoryRegionAddress,
    host_page_size, u64_to_usize, usize_to_u64,
};
use crate::vstate::snapshot::{RegionMeta, Run, SnapshotError, read_working_set};

/// The kernel's per-page state, one 8-byte entry per host page of this process's address space.
const PAGEMAP: &str = "/proc/self/pagemap";

/// This process's pagemap, opened before the process entered its jail (a jailed VMM's new root has no
/// `/proc`). The descriptor keeps naming this process after `pivot_root`.
static EARLY_PAGEMAP: std::sync::OnceLock<File> = std::sync::OnceLock::new();

/// Opens this process's pagemap now, for snapshots taken after the process is jailed (see
/// [`EARLY_PAGEMAP`]). The builder calls it before it enters the jail; a failure is left for the
/// snapshot to report.
pub fn open_pagemap_early() {
    if EARLY_PAGEMAP.get().is_none()
        && let Ok(file) = File::open(PAGEMAP)
    {
        let _ = EARLY_PAGEMAP.set(file);
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

/// Checks that `regions` (from [`layout`]) describe `mem`: one region per guest region.
fn check_layout(mem: &GuestMemoryMmap, regions: &[RegionMeta]) -> Result<(), SnapshotError> {
    let in_memory = mem.iter().count();
    if regions.len() != in_memory {
        return Err(SnapshotError::Mismatch(format!(
            "the layout has {} regions and the memory has {in_memory}",
            regions.len()
        )));
    }
    Ok(())
}

/// The memory file's length once every region of `regions` is written.
fn layout_end(regions: &[RegionMeta]) -> u64 {
    regions
        .iter()
        .map(|meta| meta.offset.saturating_add(meta.len))
        .max()
        .unwrap_or(0)
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
    check_layout(mem, regions)?;
    // A full snapshot's file holds the touched pages and holes everywhere else, so content left in
    // a reused file from an earlier snapshot must not survive. A diff keeps its base copy.
    if matches!(kind, SnapshotKind::Full) {
        file.set_len(0)
            .map_err(|source| io_error(MEMORY_FILE, source))?;
    }
    let written = walk(mem, regions, kind, &mut Sink::File(file))?;
    file.set_len(layout_end(regions))
        .map_err(|source| io_error(MEMORY_FILE, source))?;
    Ok(written)
}

/// The guest memory a capture keeps in RAM: the selected pages, copied, each with its offset in the
/// memory file. [`write_captured`] writes it to the file.
pub struct CapturedMemory {
    pieces: Vec<(u64, Vec<u8>)>,
    /// The bytes of memory captured.
    pub bytes: u64,
    /// The memory file's length once written (the end of the layout).
    len: u64,
}

/// Copies the pages [`write_memory`] would write into RAM, returning `None` instead when they would
/// pass `limit` bytes: the caller then writes the file at once. Called with the VM paused, like
/// [`write_memory`]; the copy is the only part of a capture that the pause covers.
pub fn capture_memory(
    mem: &GuestMemoryMmap,
    regions: &[RegionMeta],
    kind: SnapshotKind,
    limit: u64,
) -> Result<Option<CapturedMemory>, SnapshotError> {
    check_layout(mem, regions)?;
    let mut sink = Sink::Ram {
        pieces: Vec::new(),
        kept: 0,
        limit,
        over: false,
    };
    let bytes = walk(mem, regions, kind, &mut sink)?;
    Ok(match sink {
        Sink::Ram {
            pieces,
            over: false,
            ..
        } => Some(CapturedMemory {
            pieces,
            bytes,
            len: layout_end(regions),
        }),
        _ => None,
    })
}

/// Writes what [`capture_memory`] kept into `file`: a full snapshot's file is emptied first, each
/// piece goes to its offset, and the file's length is set last. Returns the bytes of memory written.
pub fn write_captured(
    captured: &CapturedMemory,
    file: &File,
    kind: SnapshotKind,
) -> Result<u64, SnapshotError> {
    if matches!(kind, SnapshotKind::Full) {
        file.set_len(0)
            .map_err(|source| io_error(MEMORY_FILE, source))?;
    }
    for (offset, bytes) in &captured.pieces {
        file.write_all_at(bytes, *offset)
            .map_err(|source| io_error(MEMORY_FILE, source))?;
    }
    file.set_len(captured.len)
        .map_err(|source| io_error(MEMORY_FILE, source))?;
    Ok(captured.bytes)
}

/// Where the pages a walk selects go.
enum Sink<'a> {
    /// The memory file, at the pages' offsets.
    File(&'a File),
    /// RAM. Pages are copied until they would pass `limit` bytes; from then on nothing is copied and
    /// the walk only counts, so `over` is set and the pieces are dropped.
    Ram {
        pieces: Vec<(u64, Vec<u8>)>,
        kept: u64,
        limit: u64,
        over: bool,
    },
}

/// Walks `/proc/self/pagemap` over every region of `mem` and sends the pages `kind` selects to
/// `sink`. Returns the bytes selected, whatever the sink did with them.
fn walk(
    mem: &GuestMemoryMmap,
    regions: &[RegionMeta],
    kind: SnapshotKind,
    sink: &mut Sink,
) -> Result<u64, SnapshotError> {
    let page = usize_to_u64(host_page_size());
    let opened;
    let pagemap = match EARLY_PAGEMAP.get() {
        Some(file) => file,
        None => {
            opened = File::open(PAGEMAP).map_err(|source| io_error(PAGEMAP, source))?;
            &opened
        }
    };
    let mut written = 0;
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
        written += write_region(region, meta, pagemap, sink, kind, &mut buf)?;
    }
    Ok(written)
}

/// Sends the selected pages of one region to `sink`: the pagemap is read in batches, and each run
/// of adjacent selected pages goes in one piece.
fn write_region(
    region: &GuestRegionMmapExt,
    meta: &RegionMeta,
    pagemap: &File,
    sink: &mut Sink,
    kind: SnapshotKind,
    buf: &mut Vec<u8>,
) -> Result<u64, SnapshotError> {
    let page = usize_to_u64(host_page_size());
    let host = region
        .get_host_address(MemoryRegionAddress(0))
        .map_err(|e| SnapshotError::State(format!("guest memory has no host address: {e}")))?;
    let host = usize_to_u64(host as usize);
    if !host.is_multiple_of(page) {
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
        for (i, chunk) in entries[..bytes].as_chunks::<8>().0.iter().enumerate() {
            let page_index = done + usize_to_u64(i);
            if selected(pagemap_entry(chunk), kind) {
                match run {
                    Some((start, count)) if start + count == page_index => {
                        run = Some((start, count + 1));
                    }
                    _ => {
                        written += flush_run(&mut run, region, meta, sink, buf)?;
                        run = Some((page_index, 1));
                    }
                }
            } else {
                written += flush_run(&mut run, region, meta, sink, buf)?;
            }
        }
        done += batch;
    }
    written += flush_run(&mut run, region, meta, sink, buf)?;
    Ok(written)
}

/// Sends the pending run of pages to `sink`, if there is one, and returns the bytes selected.
fn flush_run(
    run: &mut Option<(u64, u64)>,
    region: &GuestRegionMmapExt,
    meta: &RegionMeta,
    sink: &mut Sink,
    buf: &mut Vec<u8>,
) -> Result<u64, SnapshotError> {
    match run.take() {
        Some((start, count)) => send_pages(region, meta, sink, start, count, buf),
        None => Ok(0),
    }
}

/// Sends `count` pages of `region` from its page `start` to `sink`, at the region's offset in the
/// memory file, in pieces of at most [`MAX_WRITE_BYTES`].
fn send_pages(
    region: &GuestRegionMmapExt,
    meta: &RegionMeta,
    sink: &mut Sink,
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
        let file_offset = meta.offset + guest_offset;
        match sink {
            Sink::File(file) => {
                buf.resize(u64_to_usize(n * page), 0);
                region
                    .read_slice(buf.as_mut_slice(), MemoryRegionAddress(guest_offset))
                    .map_err(|e| SnapshotError::State(format!("cannot read guest memory: {e}")))?;
                file.write_all_at(buf.as_slice(), file_offset)
                    .map_err(|source| io_error(MEMORY_FILE, source))?;
            }
            Sink::Ram {
                pieces,
                kept,
                limit,
                over,
            } => {
                if !*over && *kept + n * page > *limit {
                    *over = true;
                    *kept = 0;
                    pieces.clear();
                }
                if !*over {
                    let mut piece = vec![0u8; u64_to_usize(n * page)];
                    region
                        .read_slice(piece.as_mut_slice(), MemoryRegionAddress(guest_offset))
                        .map_err(|e| {
                            SnapshotError::State(format!("cannot read guest memory: {e}"))
                        })?;
                    pieces.push((file_offset, piece));
                    *kept += n * page;
                }
            }
        }
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
        SnapshotKind::Diff | SnapshotKind::DiffUnsynced => swapped || (present && !file_backed),
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

/// The most bytes of the memory file a restore asks the kernel to read ahead. The hint is bounded, so a
/// large working set cannot queue more I/O than the VM is likely to need soon.
const PREFETCH_BYTES: u64 = 2 << 30;

/// The memory-file runs of a region's resident pages, from its `mincore` vector (see
/// [`crate::sys::resident_pages`]): one byte per page of the region, its low bit set for a resident
/// page. `region_offset` is where the region starts in the memory file and `page` is the host page
/// size. Adjacent resident pages become one run.
pub fn resident_runs(vec: &[u8], region_offset: u64, page: u64) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for (index, state) in vec.iter().enumerate() {
        if state & 1 == 0 {
            continue;
        }
        let offset = region_offset.saturating_add(index as u64 * page);
        match runs.last_mut() {
            Some(last) if last.0.saturating_add(last.1) == offset => last.1 += page,
            _ => runs.push((offset, page)),
        }
    }
    runs
}

/// The leading part of `runs` that fits in `budget` bytes, with the last run cut to fit. The order of
/// `runs` is kept.
pub fn prefetch_ranges(runs: &[Run], budget: u64) -> Vec<Run> {
    let mut left = budget;
    let mut ranges = Vec::new();
    for &(offset, len) in runs {
        if left == 0 {
            break;
        }
        let len = len.min(left);
        ranges.push((offset, len));
        left -= len;
    }
    ranges
}

/// Starts reading the working set of the snapshot in `dir` into the page cache, for the memory file
/// `memory` that a restore has just mapped: `posix_fadvise(WILLNEED)` on each hinted range, up to
/// [`PREFETCH_BYTES`]. The advice returns without waiting for the I/O. A missing or malformed hint
/// means no read-ahead, and a failed advice stops the rest silently: the hint only saves time.
pub fn read_ahead(dir: &Path, memory: &File) {
    for (offset, len) in prefetch_ranges(&read_working_set(dir), PREFETCH_BYTES) {
        if crate::sys::willneed(memory, offset, len).is_err() {
            break;
        }
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

    /// Residency bits become memory-file runs: adjacent resident pages merge, a gap splits them, and
    /// only the low bit counts.
    #[test]
    fn resident_pages_become_runs_of_file_offsets() {
        let page = 4096;
        let base = 8 * page;
        // Pages 0 and 1, then 3 to 5, then 8; the byte 2 has its low bit clear, 3 has it set.
        let vec = [1u8, 1, 2, 3, 1, 1, 0, 0, 1];
        assert_eq!(
            resident_runs(&vec, base, page),
            vec![
                (base, 2 * page),
                (base + 3 * page, 3 * page),
                (base + 8 * page, page)
            ]
        );
        assert_eq!(resident_runs(&[0u8; 4], base, page), Vec::<Run>::new());
        assert_eq!(resident_runs(&[], base, page), Vec::<Run>::new());
    }

    /// The read-ahead is cut at its budget, and the last run is shortened to fit it.
    #[test]
    fn prefetch_ranges_stay_within_the_budget() {
        let runs = [(0, 4096), (8192, 8192), (65536, 4096)];
        assert_eq!(prefetch_ranges(&runs, 1 << 20), runs.to_vec());
        assert_eq!(prefetch_ranges(&runs, 6000), vec![(0, 4096), (8192, 1904)]);
        assert_eq!(prefetch_ranges(&runs, 0), Vec::<Run>::new());
    }

    /// A restore reads a working-set hint ahead, and a missing or malformed one is no error: the
    /// memory file it names is left alone.
    #[test]
    fn read_ahead_tolerates_a_missing_or_malformed_hint() {
        let dir = scratch("read-ahead");
        let memory_path = dir.join("memory.img");
        let memory = create(&memory_path);
        memory.set_len(1 << 16).unwrap();
        read_ahead(&dir, &memory);
        fs::write(
            dir.join(crate::vstate::snapshot::WORKING_SET_FILE),
            [7u8; 20],
        )
        .unwrap();
        read_ahead(&dir, &memory);
        fs::write(
            dir.join(crate::vstate::snapshot::WORKING_SET_FILE),
            crate::vstate::snapshot::encode_runs(&[(0, 4096), (8192, 4096)]),
        )
        .unwrap();
        read_ahead(&dir, &memory);
        assert_eq!(fs::metadata(&memory_path).unwrap().len(), 1 << 16);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// A capture holds the pages `write_memory` would write, copied at the time of the capture: the
    /// file [`write_captured`] writes from it is the file `write_memory` writes, even after the
    /// memory changed. A capture past its limit holds nothing, so the caller writes the file itself.
    #[test]
    fn a_capture_writes_the_file_write_memory_would_and_refuses_past_its_limit() {
        let mem = memory(8);
        let page = host_page_size();
        for index in [1, 2, 6] {
            fill_page(&mem, index, 0xCD);
        }
        let regions = layout(&mem);
        let dir = scratch("capture");

        let direct = dir.join("direct");
        let written = write_memory(&mem, &regions, &create(&direct), SnapshotKind::Full).unwrap();

        let captured = capture_memory(&mem, &regions, SnapshotKind::Full, 1 << 20)
            .unwrap()
            .expect("the pages fit the limit");
        assert_eq!(captured.bytes, written);
        // The guest writes on after the capture: the captured pages do not change with it.
        fill_page(&mem, 1, 0xEE);
        let via_capture = dir.join("via-capture");
        assert_eq!(
            write_captured(&captured, &create(&via_capture), SnapshotKind::Full).unwrap(),
            written
        );
        assert_eq!(
            fs::read(&via_capture).unwrap(),
            fs::read(&direct).unwrap(),
            "the captured file matches the direct one"
        );

        assert!(
            capture_memory(
                &mem,
                &regions,
                SnapshotKind::Full,
                usize_to_u64(2 * page) - 1
            )
            .unwrap()
            .is_none(),
            "three pages do not fit in a limit of two pages"
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
