// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: the jailer's raw-syscall boundary. Firecracker's
// jailer reaches the same kernel interfaces through `libc`/`vmm_sys_util` in
// `src/jailer/src/chroot.rs` and `src/jailer/src/env.rs`; this module wraps the
// same calls behind one small, SAFETY-documented surface so the safe Rust in
// [`super::jailer`] can stay `unsafe`-free.

// Syscall boundary: this module is one of the crate's enumerated
// `#[allow(unsafe_code)]` modules (see `lib.rs`). It exists so that the C calls
// the jailer needs — `unshare(2)`, `mount(2)`, `pivot_root(2)`, `chdir(2)`,
// `mkdir(2)`, `rmdir(2)`, `umount2(2)` and the `setresgid(2)`/`setresuid(2)`
// privilege drop — do not force an
// `#[allow(unsafe_code)]` onto [`super::jailer`], which is otherwise entirely
// safe. The calls mirror Firecracker's jailer one for one
// (`src/jailer/src/chroot.rs`: `unshare(CLONE_NEWNS)`,
// `mount(NULL, "/", NULL, MS_REC|MS_PRIVATE, NULL)`, the bind-over-itself,
// `pivot_root`, `chdir("/")`, `umount2(MNT_DETACH)`; `src/jailer/src/env.rs`:
// the uid/gid drop), with this port's own layout.
#![allow(unsafe_code)]

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// `CLONE_NEWNS` (`sched.h`): a new mount namespace.
pub const CLONE_NEWNS: libc::c_int = libc::CLONE_NEWNS;
/// `CLONE_NEWUSER` (`sched.h`): a new user namespace.
pub const CLONE_NEWUSER: libc::c_int = libc::CLONE_NEWUSER;

/// `MS_BIND` (`sys/mount.h`): bind-mount the source over the target.
pub const MS_BIND: libc::c_ulong = libc::MS_BIND;
/// `MS_REC`: recurse into submounts.
pub const MS_REC: libc::c_ulong = libc::MS_REC;
/// `MS_PRIVATE`: make the mount private (no propagation to the host).
pub const MS_PRIVATE: libc::c_ulong = libc::MS_PRIVATE;
/// `MS_RDONLY`: mount read-only.
pub const MS_RDONLY: libc::c_ulong = libc::MS_RDONLY;

/// `MNT_DETACH` (`sys/mount.h`): lazily unmount (what Firecracker's jailer uses
/// to drop `old_root` after `pivot_root`).
pub const MNT_DETACH: libc::c_int = libc::MNT_DETACH;

/// A path as the NUL-terminated C string the kernel wants. A path with an
/// interior NUL byte — impossible from `OsStr` in practice, but this is not
/// allowed to panic — is `InvalidInput`.
fn c(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("a jail path contains an interior NUL byte: {path:?}"),
        )
    })
}

/// An optional string as a C string (`mount`'s `fstype`/`data`, which may be
/// NULL).
fn c_opt(value: Option<&str>) -> io::Result<Option<CString>> {
    value
        .map(|value| {
            CString::new(value).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "a mount field contains a NUL byte",
                )
            })
        })
        .transpose()
}

/// `unshare(2)`: disassociate part of the caller's execution context.
///
/// `CLONE_NEWUSER` additionally requires the caller to be single-threaded —
/// the kernel implies `CLONE_THREAD` for it and refuses a non-empty thread
/// group with `EINVAL` (verified on this host's kernel: a `pthread_create`d
/// sibling makes the call fail `EINVAL`, and the same call in a single-threaded
/// process succeeds). That refusal is what [`super::jailer`] reports as
/// "this process cannot be jailed".
pub fn unshare(flags: libc::c_int) -> io::Result<()> {
    // SAFETY: `unshare(2)` takes an integer flag word and touches no memory
    // this process owns. It only changes the calling task's namespaces.
    let rc = unsafe { libc::unshare(flags) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `mount(2)`. `source` and `fstype` may be `None` (a NULL pointer), which is
/// what `mount(NULL, "/", NULL, MS_REC|MS_PRIVATE, NULL)` needs.
pub fn mount(
    source: Option<&Path>,
    target: &Path,
    fstype: Option<&str>,
    flags: libc::c_ulong,
    data: Option<&str>,
) -> io::Result<()> {
    let source = source.map(c).transpose()?;
    let target = c(target)?;
    let fstype = c_opt(fstype)?;
    let data = c_opt(data)?;
    let source = source.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());
    let fstype = fstype.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());
    let data = data
        .as_ref()
        .map_or(std::ptr::null(), |d| d.as_ptr().cast::<libc::c_void>());
    // SAFETY: every pointer is either NULL or the `as_ptr()` of a `CString`
    // that lives until the end of this statement — the kernel copies each
    // string before returning, so nothing outlives the call.
    let rc = unsafe { libc::mount(source, target.as_ptr(), fstype, flags, data) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `umount2(2)`.
pub fn umount2(target: &Path, flags: libc::c_int) -> io::Result<()> {
    let target = c(target)?;
    // SAFETY: `umount2(2)` reads one NUL-terminated string, which lives until
    // the end of this statement.
    let rc = unsafe { libc::umount2(target.as_ptr(), flags) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `pivot_root(2)`: make `new_root` the mount namespace's root, moving the old
/// root under `put_old`. There is no `libc` wrapper, so this is the raw
/// syscall (the same reason Firecracker's jailer keeps the numbers itself).
pub fn pivot_root(new_root: &Path, put_old: &Path) -> io::Result<()> {
    let new_root = c(new_root)?;
    let put_old = c(put_old)?;
    // SAFETY: `pivot_root(2)` reads two NUL-terminated strings, both of which
    // live until the end of this statement. The variadic arguments are
    // pointers, exactly the two `const char *` the syscall takes.
    let rc = unsafe { libc::syscall(libc::SYS_pivot_root, new_root.as_ptr(), put_old.as_ptr()) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `chdir(2)`.
pub fn chdir(path: &Path) -> io::Result<()> {
    let path = c(path)?;
    // SAFETY: `chdir(2)` reads one NUL-terminated string, which lives until the
    // end of this statement.
    let rc = unsafe { libc::chdir(path.as_ptr()) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `mkdir(2)`.
pub fn mkdir(path: &Path, mode: libc::mode_t) -> io::Result<()> {
    let path = c(path)?;
    // SAFETY: `mkdir(2)` reads one NUL-terminated string, which lives until the
    // end of this statement.
    let rc = unsafe { libc::mkdir(path.as_ptr(), mode) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `rmdir(2)`.
pub fn rmdir(path: &Path) -> io::Result<()> {
    let path = c(path)?;
    // SAFETY: `rmdir(2)` reads one NUL-terminated string, which lives until the
    // end of this statement.
    let rc = unsafe { libc::rmdir(path.as_ptr()) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The calling process's real uid/gid (`getuid(2)`/`getgid(2)`).
pub fn ids() -> (u32, u32) {
    // SAFETY: both calls take no arguments and return a value the kernel
    // computes for the calling task.
    unsafe { (libc::getuid(), libc::getgid()) }
}

/// The calling process's effective uid/gid (`geteuid(2)`/`getegid(2)`).
pub fn effective_ids() -> (u32, u32) {
    // SAFETY: as [`ids`].
    unsafe { (libc::geteuid(), libc::getegid()) }
}

/// `setresgid(2)`: drop the real, effective and saved gid. Called before
/// [`setresuid`] so the privilege to set the gid is still held.
pub fn setresgid(gid: u32) -> io::Result<()> {
    // SAFETY: `setresgid(2)` takes three integer arguments and touches no
    // memory this process owns.
    let rc = unsafe { libc::setresgid(gid, gid, gid) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// `setresuid(2)`: drop the real, effective and saved uid. Irreversible, which
/// is the point.
pub fn setresuid(uid: u32) -> io::Result<()> {
    // SAFETY: as [`setresgid`].
    let rc = unsafe { libc::setresuid(uid, uid, uid) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
