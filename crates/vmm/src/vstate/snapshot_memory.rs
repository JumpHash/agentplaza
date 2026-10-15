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

use crate::spawner::SnapshotKind;
use crate::vstate::memory::GuestMemoryMmap;
use crate::vstate::snapshot::{RegionMeta, SnapshotError};

/// The memory file layout of `mem`: one [`RegionMeta`] per region, in guest-address order.
pub fn layout(mem: &GuestMemoryMmap) -> Vec<RegionMeta> {
    let _ = mem;
    todo!("snapshot memory layout")
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
    let _ = (mem, regions, file, kind);
    todo!("snapshot memory write")
}
