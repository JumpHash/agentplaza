// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0

//! Copies of VM disk and memory images that share the source's blocks where the filesystem can.
//!
//! [`clone_file`] shares the source's blocks where the filesystem can: `ioctl(FICLONE)` on Linux (XFS, Btrfs,
//! bcachefs), `fclonefileat(2)` on macOS (APFS), and `FSCTL_DUPLICATE_EXTENTS_TO_FILE` on Windows (ReFS and Dev
//! Drives, PLAN 16.2, W43). The copy takes no space until either side is written, and is what makes per-VM workspace
//! copies, snapshots and forks cheap (CubeSandbox's `cubecow` does the same). Anywhere else it falls back to
//! [`crate::spawner::copy_sparse`], which keeps holes as holes. [`reflink_supported`] probes a directory once, so a
//! daemon can say at start which path its copies take.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// How a copy was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyMode {
    /// The copy shares the source's blocks (`FICLONE`, `clonefile`, block cloning).
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

/// The `clonefile` errors that mean "this filesystem or pair of files cannot share blocks" (HFS+, a network volume,
/// another volume): the copy falls back to a sparse copy.
#[cfg(target_os = "macos")]
const NOT_CLONEABLE: [i32; 3] = [libc::ENOTSUP, libc::EXDEV, libc::EINVAL];

/// Copies `from` to `to` (created or replaced), sharing blocks when the filesystem supports it and by a sparse copy
/// otherwise.
pub fn clone_file(from: &Path, to: &Path) -> io::Result<CopyMode> {
    #[cfg(target_os = "linux")]
    {
        if try_reflink(from, to)? {
            return Ok(CopyMode::Reflink);
        }
    }
    #[cfg(target_os = "macos")]
    {
        let src = fs::File::open(from)?;
        if try_clonefile(&src, to)? {
            return Ok(CopyMode::Reflink);
        }
    }
    #[cfg(windows)]
    {
        if try_block_clone(from, to)? {
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

/// Clones the open `src` to a new `to` with `fclonefileat` (APFS): `to` is removed first, since a clone needs a path
/// that does not exist. `Ok(false)` when the filesystem cannot ([`NOT_CLONEABLE`]); the caller then copies.
#[cfg(target_os = "macos")]
fn try_clonefile(src: &fs::File, to: &Path) -> io::Result<bool> {
    match fs::remove_file(to) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    match sys::fclonefile(src, to) {
        Ok(()) => Ok(true),
        Err(err)
            if err
                .raw_os_error()
                .is_some_and(|code| NOT_CLONEABLE.contains(&code)) =>
        {
            Ok(false)
        }
        Err(err) => Err(err),
    }
}

/// Block-clones `from` into a new `to` (ReFS, Dev Drives). `Ok(false)` when the volume cannot; the caller then copies.
#[cfg(windows)]
fn try_block_clone(from: &Path, to: &Path) -> io::Result<bool> {
    let src = fs::File::open(from)?;
    let dst = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(to)?;
    match sys::duplicate_extents(&src, &dst, to) {
        Ok(()) => Ok(true),
        Err(err) if sys::not_cloneable(&err) => Ok(false),
        Err(err) => Err(err),
    }
}

/// [`clone_file`] from a file already open: `to` is created (or replaced) and gets `from`'s
/// contents by sharing blocks where the filesystem allows, else by a sparse copy. A jailed VMM copies the
/// memory file it was restored from this way, through the descriptor it holds.
pub fn clone_open_file(from: &fs::File, to: &Path) -> io::Result<CopyMode> {
    #[cfg(target_os = "macos")]
    {
        if try_clonefile(from, to)? {
            return Ok(CopyMode::Reflink);
        }
    }
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
    #[cfg(windows)]
    {
        match sys::duplicate_extents(from, &dst, to) {
            Ok(()) => return Ok(CopyMode::Reflink),
            Err(err) if sys::not_cloneable(&err) => {}
            Err(err) => return Err(err),
        }
    }
    crate::spawner::copy_sparse_open(from, &dst).map(|_| CopyMode::Copy)
}

/// Whether files in `dir` can share blocks (probed with two scratch files that are removed again).
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

/// The block-sharing calls the standard library does not make: `fclonefileat(2)` on macOS and
/// `FSCTL_DUPLICATE_EXTENTS_TO_FILE` on Windows. One of the crate's enumerated `#[allow(unsafe_code)]` modules
/// (see `lib.rs`).
#[cfg(any(target_os = "macos", windows))]
#[allow(unsafe_code)]
mod sys {
    use std::io;
    use std::path::Path;

    /// Clones `src` into a new file at `dst` (which must not exist), sharing its blocks.
    #[cfg(target_os = "macos")]
    pub fn fclonefile(src: &std::fs::File, dst: &Path) -> io::Result<()> {
        use std::os::fd::AsRawFd;
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(dst.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a path with a NUL byte"))?;
        // SAFETY: `fclonefileat` reads the NUL-terminated path, which lives until the call returns, and the source
        // descriptor, borrowed from a live `File`; flags 0 asks for an ordinary clone. The result is checked.
        let rc = unsafe { libc::fclonefileat(src.as_raw_fd(), libc::AT_FDCWD, path.as_ptr(), 0) };
        if rc < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// The bytes per cluster of the volume holding `path`: block cloning works in whole clusters.
    #[cfg(windows)]
    fn cluster_bytes(path: &Path) -> io::Result<u64> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceW, GetVolumePathNameW};
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut root = vec![0u16; 1024];
        // SAFETY: both buffers are live and NUL-terminated/sized as passed; the result is checked.
        if unsafe { GetVolumePathNameW(wide.as_ptr(), root.as_mut_ptr(), root.len() as u32) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let (mut sectors, mut bytes, mut free, mut total) = (0u32, 0u32, 0u32, 0u32);
        // SAFETY: the four out-pointers are live locals; the root is the NUL-terminated buffer filled above.
        if unsafe {
            GetDiskFreeSpaceW(
                root.as_ptr(),
                &mut sectors,
                &mut bytes,
                &mut free,
                &mut total,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(u64::from(sectors) * u64::from(bytes))
    }

    /// Shares `src`'s blocks into `dst` (just created, empty, on the same ReFS volume), in whole clusters.
    #[cfg(windows)]
    pub fn duplicate_extents(
        src: &std::fs::File,
        dst: &std::fs::File,
        dst_path: &Path,
    ) -> io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_SPARSE_FILE, GetFileInformationByHandle,
        };
        use windows_sys::Win32::System::IO::DeviceIoControl;
        use windows_sys::Win32::System::Ioctl::{
            DUPLICATE_EXTENTS_DATA, FSCTL_DUPLICATE_EXTENTS_TO_FILE, FSCTL_SET_SPARSE,
        };
        const CHUNK: u64 = 1 << 30;
        let len = src.metadata()?.len();
        let cluster = cluster_bytes(dst_path)?.max(4096);
        let rounded = len.div_ceil(cluster) * cluster;
        // SAFETY: the out-structure is a live local the call fills; the handle is borrowed from a live `File`.
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(src.as_raw_handle(), &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut returned = 0u32;
        if info.dwFileAttributes & FILE_ATTRIBUTE_SPARSE_FILE != 0 {
            // A sparse source needs a sparse destination before its extents are shared.
            // SAFETY: FSCTL_SET_SPARSE takes no buffers; `returned` is a live local.
            let ok = unsafe {
                DeviceIoControl(
                    dst.as_raw_handle(),
                    FSCTL_SET_SPARSE,
                    std::ptr::null(),
                    0,
                    std::ptr::null_mut(),
                    0,
                    &mut returned,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        dst.set_len(rounded)?;
        let mut offset = 0u64;
        while offset < rounded {
            let count = CHUNK.min(rounded - offset);
            let data = DUPLICATE_EXTENTS_DATA {
                FileHandle: src.as_raw_handle(),
                SourceFileOffset: offset as i64,
                TargetFileOffset: offset as i64,
                ByteCount: count as i64,
            };
            // SAFETY: `data` is a live, fully initialised DUPLICATE_EXTENTS_DATA passed with its size; the source
            // handle in it and the destination handle are borrowed from live `File`s; there is no output buffer.
            let ok = unsafe {
                DeviceIoControl(
                    dst.as_raw_handle(),
                    FSCTL_DUPLICATE_EXTENTS_TO_FILE,
                    (&raw const data).cast(),
                    std::mem::size_of::<DUPLICATE_EXTENTS_DATA>() as u32,
                    std::ptr::null_mut(),
                    0,
                    &mut returned,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            offset += count;
        }
        dst.set_len(len)
    }

    /// Whether a block-cloning error means "this volume or pair cannot" (NTFS, another volume): the caller copies.
    #[cfg(windows)]
    pub fn not_cloneable(err: &io::Error) -> bool {
        use windows_sys::Win32::Foundation::{
            ERROR_BLOCK_TOO_MANY_REFERENCES, ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER,
            ERROR_NOT_SAME_DEVICE, ERROR_NOT_SUPPORTED,
        };
        err.raw_os_error().is_some_and(|code| {
            [
                ERROR_INVALID_FUNCTION,
                ERROR_NOT_SUPPORTED,
                ERROR_NOT_SAME_DEVICE,
                ERROR_INVALID_PARAMETER,
                ERROR_BLOCK_TOO_MANY_REFERENCES,
            ]
            .contains(&(code as u32))
        })
    }
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
        // A filesystem that clones (APFS here, a reflink one on Linux) shares the blocks; the others copy sparsely.
        let mode = clone_file(&from, &to).expect("a copy");
        assert!(
            matches!(mode, CopyMode::Copy | CopyMode::Reflink),
            "{mode:?}"
        );

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

    /// On APFS (macOS's own volumes) a copy is a clone: it shares the source's blocks, and a write to the copy does
    /// not reach the source.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_copy_on_apfs_is_a_clone_and_writes_stay_on_their_side() {
        let dir = scratch("apfs");
        assert!(reflink_supported(&dir), "the build target's volume clones");
        let from = dir.join("base.img");
        fs::write(&from, vec![7u8; 1 << 20]).expect("the source");
        let to = dir.join("copy.img");
        assert_eq!(clone_file(&from, &to).expect("a clone"), CopyMode::Reflink);
        let mut copy = fs::OpenOptions::new()
            .write(true)
            .open(&to)
            .expect("the copy");
        copy.write_all(&[9u8; 4096]).expect("a write to the copy");
        drop(copy);
        assert_eq!(fs::read(&from).expect("the source"), vec![7u8; 1 << 20]);
        let open = fs::File::open(&from).expect("the source, open");
        let again = dir.join("again.img");
        assert_eq!(
            clone_open_file(&open, &again).expect("a clone"),
            CopyMode::Reflink
        );
        assert_eq!(fs::read(&again).expect("the clone"), vec![7u8; 1 << 20]);
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
