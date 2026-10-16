// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0

//! Copies of VM disk and memory images that share the source's blocks where the filesystem can.
//!
//! On a filesystem with reflinks (XFS, Btrfs, bcachefs) [`clone_file`] is `ioctl(FICLONE)`: the copy
//! is O(extents), takes no space until either side is written, and is what makes per-VM workspace
//! copies, snapshots and forks cheap (CubeSandbox's `cubecow` does the same). Anywhere else it falls
//! back to [`crate::spawner::copy_sparse`], which keeps holes as holes. [`reflink_supported`] probes a
//! directory once, so a daemon can say at start which path its copies take.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// How a copy was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyMode {
    /// The copy shares the source's extents (`FICLONE`).
    Reflink,
    /// The bytes were copied (holes kept).
    Copy,
}

/// The `FICLONE` errors that mean "this file or filesystem cannot share extents": the copy falls
/// back to a sparse copy. Any other error is the caller's.
#[cfg(target_os = "linux")]
const NOT_REFLINKABLE: [i32; 5] = [
    libc::EOPNOTSUPP,
    libc::EXDEV,
    libc::EINVAL,
    libc::ENOTTY,
    libc::EBADF,
];

/// Copies `from` to `to` (created or truncated), by reflink when the filesystem supports it and by
/// a sparse copy otherwise.
///
/// On Linux the reflink is attempted first; `Ok(false)` from [`try_reflink`] means the filesystem
/// said no and the sparse copy below takes over. Elsewhere there is no reflink, so the sparse copy
/// is the whole of it.
pub fn clone_file(from: &Path, to: &Path) -> io::Result<CopyMode> {
    #[cfg(target_os = "linux")]
    {
        if try_reflink(from, to)? {
            return Ok(CopyMode::Reflink);
        }
    }
    crate::spawner::copy_sparse(from, to).map(|_| CopyMode::Copy)
}

/// Creates `to` with `from`'s permissions and shares `from`'s extents into it. `Ok(false)` when the
/// kernel refuses the clone for a reason in [`NOT_REFLINKABLE`]; the destination is then empty and
/// the caller re-creates it.
#[cfg(target_os = "linux")]
fn try_reflink(from: &Path, to: &Path) -> io::Result<bool> {
    let src = fs::File::open(from)?;
    let dst = fs::File::create(to)?;
    dst.set_permissions(src.metadata()?.permissions())?;
    match crate::sys::ficlone(&dst, &src) {
        Ok(()) => Ok(true),
        Err(err)
            if err
                .raw_os_error()
                .is_some_and(|code| NOT_REFLINKABLE.contains(&code)) =>
        {
            Ok(false)
        }
        Err(err) => Err(err),
    }
}

/// [`clone_file`] from a file already open: `to` is created (or truncated) and gets `from`'s
/// contents by reflink where the filesystem allows, else by a sparse copy. A jailed VMM copies the
/// memory file it was restored from this way, through the descriptor it holds.
pub fn clone_open_file(from: &fs::File, to: &Path) -> io::Result<CopyMode> {
    let dst = fs::File::create(to)?;
    #[cfg(target_os = "linux")]
    {
        dst.set_permissions(from.metadata()?.permissions())?;
        match crate::sys::ficlone(&dst, from) {
            Ok(()) => return Ok(CopyMode::Reflink),
            Err(err)
                if err
                    .raw_os_error()
                    .is_some_and(|code| NOT_REFLINKABLE.contains(&code)) => {}
            Err(err) => return Err(err),
        }
    }
    crate::spawner::copy_sparse_open(from, &dst).map(|_| CopyMode::Copy)
}

/// Whether files in `dir` can be reflinked (probed with two scratch files that are removed again).
///
/// A probe that cannot run (the directory is missing, or not writable) answers `false`: the copies
/// would take the sparse path there too, and that path reports its own error when it fails.
pub fn reflink_supported(dir: &Path) -> bool {
    let probe = |name: &str| -> PathBuf {
        dir.join(format!(".apz-reflink-probe-{}-{name}", std::process::id()))
    };
    let (from, to) = (probe("from"), probe("to"));
    let supported = fs::write(&from, b"probe").is_ok()
        && matches!(clone_file(&from, &to), Ok(CopyMode::Reflink));
    let _ = fs::remove_file(&from);
    let _ = fs::remove_file(&to);
    supported
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    /// A scratch directory under the crate's build target, unique to this process and test.
    fn scratch(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/clonefile-tests")
            .join(format!("{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// A sparse image keeps its holes in the copy: the copy has the source's length and the
    /// same bytes, and it allocates only the two blocks that were written.
    #[test]
    fn a_copy_keeps_the_holes_of_a_sparse_source() {
        let dir = scratch("holes");
        let from = dir.join("sparse.img");
        let len: u64 = 8 << 20;
        let mut file = fs::File::create(&from).expect("the source");
        file.set_len(len).expect("a sparse length");
        file.seek(SeekFrom::Start(0)).expect("the first block");
        file.write_all(&[0xAB; 4096]).expect("written");
        file.seek(SeekFrom::Start(4 << 20))
            .expect("the second block");
        file.write_all(&[0xCD; 4096]).expect("written");
        file.sync_all().expect("on disk");
        drop(file);
        // A filesystem that allocates the whole length (some macOS volumes do) has no holes to keep: the
        // copy's own correctness is covered by the other tests, so this one has nothing to check there.
        if fs::metadata(&from).expect("the source").blocks() * 512 >= 1 << 20 {
            eprintln!(
                "skipped: this filesystem allocated the sparse source in full, so it has no holes"
            );
            return;
        }

        let to = dir.join("copy.img");
        assert_eq!(clone_file(&from, &to).expect("a copy"), CopyMode::Copy);

        assert_eq!(
            fs::read(&to).expect("the copy"),
            fs::read(&from).expect("the source"),
            "the copy has the source's bytes"
        );
        let copy = fs::metadata(&to).expect("the copy");
        assert_eq!(copy.len(), len);
        assert!(
            copy.blocks() * 512 < 1 << 20,
            "the copy allocates the written blocks, not the image's length"
        );
    }

    /// A copy takes the source's permission bits.
    #[test]
    fn a_copy_keeps_the_source_permissions() {
        let dir = scratch("modes");
        let from = dir.join("base.img");
        fs::write(&from, b"an image").expect("the source");
        fs::set_permissions(&from, fs::Permissions::from_mode(0o640)).expect("its mode");
        let to = dir.join("copy.img");

        clone_file(&from, &to).expect("a copy");

        assert_eq!(
            fs::metadata(&to).expect("the copy").permissions().mode() & 0o777,
            0o640
        );
    }

    /// A copy over an existing, longer file replaces it entirely.
    #[test]
    fn a_copy_replaces_an_existing_destination() {
        let dir = scratch("replace");
        let from = dir.join("base.img");
        fs::write(&from, b"short").expect("the source");
        let to = dir.join("copy.img");
        fs::write(&to, vec![0xFFu8; 1 << 20]).expect("an older, longer file");

        clone_file(&from, &to).expect("a copy");

        assert_eq!(fs::read(&to).expect("the copy"), b"short");
    }

    /// The probe removes its scratch files, whatever it answers.
    #[test]
    fn the_reflink_probe_leaves_no_files_behind() {
        let dir = scratch("probe");

        let _ = reflink_supported(&dir);

        assert_eq!(fs::read_dir(&dir).expect("the directory").count(), 0);
    }

    /// A reflink directory (XFS, Btrfs, bcachefs) shares the source's blocks, and a write to the
    /// copy does not reach the source. Runs only when `APZ_REFLINK_DIR` names such a directory.
    #[test]
    fn a_reflink_shares_the_source_blocks_when_the_directory_supports_them() {
        let Some(root) = std::env::var_os("APZ_REFLINK_DIR") else {
            eprintln!("APZ_REFLINK_DIR is unset: the reflink test is skipped");
            return;
        };
        let dir = PathBuf::from(root).join(format!("apz-clonefile-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch directory");
        assert!(
            reflink_supported(&dir),
            "APZ_REFLINK_DIR must support reflinks"
        );

        let from = dir.join("base.img");
        fs::write(&from, vec![7u8; 1 << 20]).expect("the source");
        let to = dir.join("copy.img");
        assert_eq!(clone_file(&from, &to).expect("a copy"), CopyMode::Reflink);
        assert_eq!(fs::read(&to).expect("the copy"), vec![7u8; 1 << 20]);

        let mut copy = fs::OpenOptions::new()
            .write(true)
            .open(&to)
            .expect("the copy, writable");
        copy.write_all(&[9u8; 4096]).expect("a write to the copy");
        drop(copy);
        assert_eq!(
            fs::read(&from).expect("the source"),
            vec![7u8; 1 << 20],
            "a write to the copy does not reach the source"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
