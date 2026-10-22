//! Code shared by the macOS and Linux backends: the child handle and the rlimits.

use std::io::{self, Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};

use crate::{Limits, SandboxError};

/// A confined child on Unix: a plain [`Child`] with its streams handed out once.
pub(crate) struct Process {
    child: Child,
}

impl Process {
    pub(crate) fn id(&self) -> u32 {
        self.child.id()
    }

    pub(crate) fn stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        self.child
            .stdin
            .take()
            .map(|stream| Box::new(stream) as Box<dyn Write + Send>)
    }

    pub(crate) fn stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        self.child
            .stdout
            .take()
            .map(|stream| Box::new(stream) as Box<dyn Read + Send>)
    }

    pub(crate) fn stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        self.child
            .stderr
            .take()
            .map(|stream| Box::new(stream) as Box<dyn Read + Send>)
    }

    pub(crate) fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait()
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    pub(crate) fn kill(&mut self) -> io::Result<()> {
        self.child.kill()
    }
}

/// Spawns a prepared confined command with the caller's stdio.
pub(crate) fn run(
    mut command: Command,
    stdin: Stdio,
    stdout: Stdio,
    stderr: Stdio,
) -> Result<Process, SandboxError> {
    command.stdin(stdin).stdout(stdout).stderr(stderr);
    Ok(Process {
        child: command.spawn()?,
    })
}

/// The most descriptors the fallback loop will try to close.
const FD_SCAN_LIMIT: u64 = 65_536;

/// Closes every descriptor above stderr, so the child inherits nothing the hub holds
/// (a socket to the desktop app, a database file, ...). Std has already moved the
/// requested stdio onto 0, 1 and 2 when `pre_exec` hooks run.
///
/// Called from a `pre_exec` hook: close_range where the kernel has it, otherwise one
/// close() per descriptor up to the soft RLIMIT_NOFILE. Only async-signal-safe calls.
pub(crate) fn close_inherited_fds() {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: close_range takes plain integers and no pointers. It fails with ENOSYS
        // on kernels older than 5.9, and the loop below then does the work.
        let rc = unsafe {
            libc::syscall(
                libc::SYS_close_range,
                3 as libc::c_long,
                libc::c_uint::MAX as libc::c_long,
                0 as libc::c_long,
            )
        };
        if rc == 0 {
            return;
        }
    }
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a valid out pointer for the call.
    let scan = if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE as _, &mut limit) } == 0 {
        limit.rlim_cur.min(FD_SCAN_LIMIT)
    } else {
        FD_SCAN_LIMIT
    };
    for fd in 3..scan {
        // SAFETY: close() takes a descriptor number. An unopened one just fails with EBADF.
        unsafe { libc::close(fd as libc::c_int) };
    }
}

/// Applies the CPU, process and memory caps with setrlimit.
///
/// Called from a `pre_exec` hook, so it runs in the forked child before `exec`:
/// it makes only setrlimit calls and allocates nothing on success.
pub(crate) fn apply_rlimits(limits: &Limits) -> io::Result<()> {
    if let Some(bytes) = limits.memory_bytes {
        set_rlimit(libc::RLIMIT_AS as libc::c_int, bytes)?;
        set_rlimit(libc::RLIMIT_DATA as libc::c_int, bytes)?;
    }
    if let Some(count) = limits.processes {
        set_rlimit(libc::RLIMIT_NPROC as libc::c_int, u64::from(count))?;
    }
    if let Some(seconds) = limits.cpu_seconds {
        set_rlimit(libc::RLIMIT_CPU as libc::c_int, seconds)?;
    }
    Ok(())
}

fn set_rlimit(resource: libc::c_int, value: u64) -> io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: value as libc::rlim_t,
        rlim_max: value as libc::rlim_t,
    };
    // SAFETY: `limit` is an initialised rlimit that outlives the call, and
    // `resource` is one of the RLIMIT_* constants (converted to the libc's type).
    let rc = unsafe { libc::setrlimit(resource as _, &limit) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
