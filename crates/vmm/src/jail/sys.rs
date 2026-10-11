// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port: the seccomp-bpf install path. The shape follows
// Firecracker's `src/vmm/src/seccomp.rs::apply_filter` (set `no_new_privs`, hand
// a `struct sock_fprog` to the kernel, per thread), but the BPF program it
// installs is hand-built by [`super`] from the Linux UAPI headers
// (`linux/seccomp.h`, `linux/filter.h`, `linux/bpf_common.h`) rather than
// compiled by Firecracker's seccompiler/libseccomp, which this port does not
// vendor. Firecracker uses `seccomp(SECCOMP_SET_MODE_FILTER)`; this module uses
// the equivalent, older `prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER)`, per the
// port's stated design.

// Syscall boundary (see the allowlist in `lib.rs`): the two `prctl(2)` calls
// that install the filter, which no safe-Rust interface exposes.
#![allow(unsafe_code)]

use std::io;

/// The kernel's maximum BPF program length (`BPF_MAXINSNS`); a longer program
/// is rejected by the kernel, so it is rejected here first, with the length.
pub const BPF_MAX_LEN: usize = 4096;

/// `struct sock_fprog` from `linux/filter.h`: a program length and a pointer to
/// an array of `struct sock_filter` (8 bytes each). This port encodes one
/// instruction as a little-endian `u64` ([`super::program`]), which is exactly
/// the `{ __u16 code; __u8 jt; __u8 jf; __u32 k; }` layout on a little-endian
/// host — the alignment is even stricter than the kernel's, so the pointer is
/// valid for the kernel to read.
#[repr(C)]
struct SockFprog {
    /// Number of instructions in `filter`.
    len: libc::c_ushort,
    /// The instructions themselves.
    filter: *const u64,
}

/// Installs `program` on the calling thread: `no_new_privs` first, then the
/// filter. No `TSYNC`: the filter applies to the calling thread only, which is
/// what the per-thread arming in this crate relies on (see [`super`]).
///
/// Fails closed: if either `prctl(2)` fails the filter is not installed and the
/// error is returned, so the caller can refuse to run rather than run
/// unconfined. (A partial install cannot happen the other way round: the
/// `no_new_privs` bit is harmless on its own, and
/// `PR_SET_SECCOMP`+`SECCOMP_MODE_FILTER` is atomic.)
pub fn install(program: &[u64]) -> io::Result<()> {
    if program.len() > BPF_MAX_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "the seccomp program is {} instructions, over the kernel's {BPF_MAX_LEN}",
                program.len()
            ),
        ));
    }
    let prog = SockFprog {
        // `program.len() <= BPF_MAX_LEN` (4096) fits `c_ushort`.
        #[allow(clippy::cast_possible_truncation)]
        len: program.len() as libc::c_ushort,
        filter: program.as_ptr(),
    };

    // SAFETY: `PR_SET_NO_NEW_PRIVS` takes an integer argument; the remaining
    // variadic slots are unused and passed as 0, as the kernel requires. The
    // call is async-signal-safe and cannot touch this process's memory.
    let rc = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: `PR_SET_SECCOMP` with `SECCOMP_MODE_FILTER` reads a
    // `struct sock_fprog` through the third argument. `prog` is a correctly
    // laid out, private `SockFprog` that lives for the whole call, and its
    // `filter` pointer targets `program`, a borrowed slice the caller keeps
    // alive across this call. The kernel copies the program before returning,
    // so neither the slice nor `prog` outlives its use here.
    let rc = unsafe {
        libc::prctl(
            libc::PR_SET_SECCOMP,
            libc::SECCOMP_MODE_FILTER,
            &prog as *const SockFprog,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}
