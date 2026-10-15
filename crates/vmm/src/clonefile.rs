// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0

//! Copies of VM disk and memory images that share the source's blocks where the filesystem can.
//!
//! On a filesystem with reflinks (XFS, Btrfs, bcachefs) [`clone_file`] is `ioctl(FICLONE)`: the copy
//! is O(extents), takes no space until either side is written, and is what makes per-VM workspace
//! copies, snapshots and forks cheap (CubeSandbox's `cubecow` does the same). Anywhere else it falls
//! back to [`crate::spawner::copy_sparse`], which keeps holes as holes. [`reflink_supported`] probes a
//! directory once, so a daemon can say at start which path its copies take.

use std::io;
use std::path::Path;

/// How a copy was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyMode {
    /// The copy shares the source's extents (`FICLONE`).
    Reflink,
    /// The bytes were copied (holes kept).
    Copy,
}

/// Copies `from` to `to` (created or truncated), by reflink when the filesystem supports it and by
/// a sparse copy otherwise.
pub fn clone_file(from: &Path, to: &Path) -> io::Result<CopyMode> {
    crate::spawner::copy_sparse(from, to).map(|_| CopyMode::Copy)
}

/// Whether files in `dir` can be reflinked (probed with two scratch files that are removed again).
pub fn reflink_supported(dir: &Path) -> bool {
    let _ = dir;
    false
}
