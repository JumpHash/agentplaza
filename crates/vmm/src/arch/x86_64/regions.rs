// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//
// Derived from Firecracker src/vmm/src/arch/x86_64/mod.rs and arch/mod.rs:
// `arch_memory_regions` and its gap helper, moved to their own file so the
// Windows (WHP) backend, which does not build the KVM half of this module,
// lays guest RAM out exactly as the KVM backend does.

use super::layout::{MMIO32_MEM_SIZE, MMIO32_MEM_START, MMIO64_MEM_SIZE, MMIO64_MEM_START};
use crate::vstate::memory::{GuestAddress, u64_to_usize};

/// Adds in [`regions`] the valid memory regions suitable for RAM taking into account a gap in the
/// available address space and returns the remaining region (if any) past this gap
/// (upstream: `arch/mod.rs`).
fn arch_memory_regions_with_gap(
    regions: &mut Vec<(GuestAddress, usize)>,
    region_start: usize,
    region_size: usize,
    gap_start: usize,
    gap_size: usize,
) -> Option<(usize, usize)> {
    // 0-sized gaps don't really make sense. We should never receive such a gap.
    assert!(gap_size > 0);

    let first_addr_past_gap = gap_start + gap_size;
    match (region_start + region_size).checked_sub(gap_start) {
        // case0: region fits all before gap
        None | Some(0) => {
            regions.push((GuestAddress(region_start as u64), region_size));
            None
        }
        // case1: region starts before the gap and goes past it
        Some(remaining) if region_start < gap_start => {
            regions.push((GuestAddress(region_start as u64), gap_start - region_start));
            Some((first_addr_past_gap, remaining))
        }
        // case2: region starts past the gap
        Some(_) => Some((first_addr_past_gap.max(region_start), region_size)),
    }
}

/// Returns a Vec of the valid memory addresses.
/// These should be used to configure the GuestMemoryMmap structure for the platform.
/// For x86_64 all addresses are valid from the start of the kernel except an 1GB
/// carve out at the end of 32bit address space and a second 256GB one at the 256GB limit.
pub fn arch_memory_regions(size: usize) -> Vec<(GuestAddress, usize)> {
    // If we get here with size == 0 something has seriously gone wrong. Firecracker should never
    // try to allocate guest memory of size 0
    assert!(size > 0, "Attempt to allocate guest memory of length 0");

    let dram_size = std::cmp::min(
        usize::MAX - u64_to_usize(MMIO32_MEM_SIZE) - u64_to_usize(MMIO64_MEM_SIZE),
        size,
    );
    if dram_size != size {
        eprintln!(
            "warning: requested memory size {size} exceeds architectural maximum (1022GiB); \
             size has been truncated to {dram_size}"
        );
    }

    let mut regions = vec![];

    if let Some((start_past_32bit_gap, remaining_past_32bit_gap)) = arch_memory_regions_with_gap(
        &mut regions,
        0,
        dram_size,
        MMIO32_MEM_START as usize,
        MMIO32_MEM_SIZE as usize,
    ) && let Some((start_past_64bit_gap, remaining_past_64bit_gap)) =
        arch_memory_regions_with_gap(
            &mut regions,
            start_past_32bit_gap,
            remaining_past_32bit_gap,
            MMIO64_MEM_START as usize,
            MMIO64_MEM_SIZE as usize,
        )
    {
        regions.push((
            GuestAddress(start_past_64bit_gap as u64),
            remaining_past_64bit_gap,
        ));
    }

    regions
}
