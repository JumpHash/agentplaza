//! Process hardening for the vault. A crash must not leave the keys in a core file, and a debugger
//! must not attach to read them. On Linux in release builds apid clears the dumpable flag, which
//! also makes its `/proc/<pid>` memory and environment unreadable to other users.

/// Clears the dumpable flag on Linux release builds. Elsewhere, and in debug builds, does nothing.
#[cfg(all(target_os = "linux", not(debug_assertions)))]
#[allow(unsafe_code)]
pub fn harden() {
    // SAFETY: prctl(PR_SET_DUMPABLE, 0) takes plain integers and touches no memory of ours.
    let rc = unsafe {
        libc::prctl(
            libc::PR_SET_DUMPABLE,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
        )
    };
    if rc != 0 {
        eprintln!(
            "apid: cannot clear the dumpable flag: {}",
            std::io::Error::last_os_error()
        );
    }
}

/// Nothing to do on this platform or in a debug build.
#[cfg(not(all(target_os = "linux", not(debug_assertions))))]
pub fn harden() {}
