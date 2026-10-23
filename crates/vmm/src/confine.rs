// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//! The confinement of a VMM process (PLAN 20.3): what a monitor process may read, write, reach and use.
//!
//! On Linux a per-VM process runs under seccomp and the jailer; on Windows the VMM runs as threads of `noded` with
//! the user's rights unless its process is confined. This module builds the spec that confines it: the images it
//! boots (read-only), its state directory where the vsock sockets live (read-write), no network (a monitor has no
//! network device by design), one process (a job's active-process cap) and a memory cap of the guest's RAM plus
//! the monitor's own overhead. The OS side is `apz_sandbox`: an AppContainer and a job object on Windows, Seatbelt
//! on macOS, Landlock on Linux. The spec is platform-independent, so its tests run on every host; the enforcement is
//! `apz_sandbox::spawn` on Windows and is not run here (see docs/windows-whp.md, "Confinement").

use std::path::{Path, PathBuf};

use apz_sandbox::{Limits, Network, Spec};

/// Memory a monitor process needs beyond the guest's RAM: its code, its device state, the vCPU threads' stacks and
/// the Rust heap. The memory cap is the guest RAM plus this.
pub const MONITOR_OVERHEAD_BYTES: u64 = 256 << 20;

/// The confinement spec of one VMM process.
///
/// * `program`, `args`, `env`: the monitor binary and its whole environment (nothing is inherited).
/// * `workdir`: the monitor's cwd (created if missing); the caller passes its state directory.
/// * `read_only`: what it reads: the kernel, the initrd, data volumes, a restore directory.
/// * `read_write`: what it writes: the state directory with the vsock and control sockets, the
///   workspace image, a snapshot root.
/// * `guest_memory_bytes`: the guest's RAM; the memory cap is this plus [`MONITOR_OVERHEAD_BYTES`].
pub fn monitor_spec(
    program: &Path,
    args: Vec<String>,
    env: Vec<(String, String)>,
    workdir: &Path,
    read_only: &[PathBuf],
    read_write: &[PathBuf],
    guest_memory_bytes: u64,
) -> Spec {
    Spec {
        program: program.to_path_buf(),
        args,
        env,
        workdir: workdir.to_path_buf(),
        read_only: read_only.to_vec(),
        read_write: read_write.to_vec(),
        network: Network::None,
        limits: Limits {
            memory_bytes: Some(guest_memory_bytes.saturating_add(MONITOR_OVERHEAD_BYTES)),
            processes: Some(1),
            cpu_seconds: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAM: u64 = 512 << 20;

    fn spec() -> Spec {
        monitor_spec(
            Path::new("/agentplaza/bin/vmctl"),
            vec!["--vsock-cid".to_owned(), "3".to_owned()],
            vec![("PATH".to_owned(), "/usr/bin".to_owned())],
            Path::new("/agentplaza/vm3"),
            &[
                PathBuf::from("/agentplaza/guest/x86_64/vmlinux"),
                PathBuf::from("/agentplaza/guest/x86_64/initrd.gz"),
            ],
            &[PathBuf::from("/agentplaza/state/vsock")],
            RAM,
        )
    }

    #[test]
    fn the_images_are_read_only_and_the_socket_directory_is_the_only_writable_path() {
        let spec = spec();
        assert_eq!(spec.read_only.len(), 2, "the kernel and the initrd");
        assert!(
            spec.read_only
                .iter()
                .all(|p| p.ends_with("vmlinux") || p.ends_with("initrd.gz"))
        );
        assert_eq!(
            spec.read_write,
            vec![PathBuf::from("/agentplaza/state/vsock")]
        );
    }

    #[test]
    fn the_monitor_has_no_network_and_one_process() {
        let spec = spec();
        assert_eq!(spec.network, Network::None);
        assert_eq!(spec.limits.processes, Some(1));
        assert_eq!(spec.limits.cpu_seconds, None);
    }

    #[test]
    fn the_memory_cap_is_the_guest_ram_plus_the_monitor_overhead() {
        assert_eq!(
            spec().limits.memory_bytes,
            Some(RAM + MONITOR_OVERHEAD_BYTES)
        );
        let huge = monitor_spec(
            Path::new("/bin/vmctl"),
            vec![],
            vec![],
            Path::new("/w"),
            &[],
            &[PathBuf::from("/s")],
            u64::MAX,
        );
        assert_eq!(
            huge.limits.memory_bytes,
            Some(u64::MAX),
            "a cap at the top of the range saturates"
        );
    }

    #[test]
    fn the_program_args_env_and_workdir_are_the_callers() {
        let spec = spec();
        assert_eq!(spec.program, PathBuf::from("/agentplaza/bin/vmctl"));
        assert_eq!(spec.args, vec!["--vsock-cid".to_owned(), "3".to_owned()]);
        assert_eq!(spec.env.len(), 1);
        assert_eq!(spec.workdir, PathBuf::from("/agentplaza/vm3"));
    }
}
