//! The memory and CPU cap of a server (PLAN 15.2: each server has one in the fork).
//!
//! On Linux the child gets `RLIMIT_AS` (its address space) and `RLIMIT_CPU` (its CPU seconds over its life) before it
//! execs. On macOS and Windows no cap set here binds: `RLIMIT_AS` is not enforced on macOS, so the servers run
//! uncapped there. [`ENFORCED`] says which case this build is.

use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Address space in bytes; 0 for none.
    pub address_space: u64,
    /// CPU seconds over the process's life; 0 for none.
    pub cpu_seconds: u64,
}

impl Limits {
    pub const NONE: Limits = Limits {
        address_space: 0,
        cpu_seconds: 0,
    };
}

/// Whether [`apply`] caps the child on this OS.
pub const ENFORCED: bool = cfg!(target_os = "linux");

/// Sets the limits on a command, to take effect in the child before it execs.
#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
pub fn apply(cmd: &mut Command, limits: Limits) {
    use std::os::unix::process::CommandExt;
    if limits == Limits::NONE {
        return;
    }
    // SAFETY: the closure runs between fork and exec. It calls only setrlimit, which is async-signal-safe, and it
    // allocates nothing.
    unsafe {
        cmd.pre_exec(move || {
            for (resource, value) in [
                (libc::RLIMIT_AS, limits.address_space),
                (libc::RLIMIT_CPU, limits.cpu_seconds),
            ] {
                if value == 0 {
                    continue;
                }
                let rl = libc::rlimit {
                    rlim_cur: value as libc::rlim_t,
                    rlim_max: value as libc::rlim_t,
                };
                if libc::setrlimit(resource, &rl) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
}

/// On macOS and Windows the limits are not set: there is no cap that binds here (see the module docs).
#[cfg(not(target_os = "linux"))]
pub fn apply(_cmd: &mut Command, _limits: Limits) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// The address-space cap as the child sees it, in kilobytes (`ulimit -v`), or `unlimited`.
    #[cfg(unix)]
    fn child_address_space(limits: Limits) -> String {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "ulimit -v"]);
        apply(&mut cmd, limits);
        let out = cmd.output().expect("sh runs");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[cfg(unix)]
    #[test]
    fn the_child_is_capped_where_the_os_enforces_it_and_only_there() {
        let seen = child_address_space(Limits {
            address_space: 4 << 30,
            cpu_seconds: 0,
        });
        if ENFORCED {
            assert_eq!(seen, (4u64 << 30 >> 10).to_string());
        } else {
            assert_eq!(
                seen, "unlimited",
                "no cap is set on this OS (macOS does not enforce RLIMIT_AS)"
            );
        }
        assert_eq!(child_address_space(Limits::NONE), "unlimited");
    }

    /// Windows has no `ulimit` to read the cap with, and sets none: a child with the limits asked for still starts and
    /// runs as it would without them.
    #[cfg(windows)]
    #[test]
    fn windows_sets_no_cap_and_the_child_still_runs() {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "exit 0"]);
        apply(
            &mut cmd,
            Limits {
                address_space: 4 << 30,
                cpu_seconds: 0,
            },
        );
        let status = cmd.status().expect("cmd runs");
        assert!(status.success(), "{status}");
    }
}
