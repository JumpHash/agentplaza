//! The host's commands, behind [`Runner`] so they can be faked. The checks run one of them: the Windows
//! Hypervisor Platform query (see `checks`). The rest of the checks read the machine directly.

use std::io;
use std::process::{Command, Stdio};

/// What a command printed, and how it ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    pub success: bool,
    /// The exit code, when the command had one.
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// The host: runs commands.
pub trait Runner {
    /// Runs `program args` with stdin closed and waits for it. `Err` only when it cannot start.
    fn output(&self, program: &str, args: &[&str]) -> io::Result<Output>;
}

/// The real host.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn output(&self, program: &str, args: &[&str]) -> io::Result<Output> {
        let out = command(program).args(args).stdin(Stdio::null()).output()?;
        Ok(Output {
            success: out.status.success(),
            code: out.status.code(),
            stdout: out.stdout,
            stderr: out.stderr,
        })
    }
}

/// A command for a probe: on Windows it opens no console window (the app has none, so each console
/// program would flash its own).
fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let cmd = Command::new(program);
    #[cfg(windows)]
    let cmd = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = cmd;
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    };
    cmd
}
