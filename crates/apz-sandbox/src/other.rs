//! Platforms without a sandbox backend. Nothing can be confined here, so nothing runs.

use std::convert::Infallible;
use std::io::{Read, Write};
use std::process::{Command, ExitStatus, Stdio};

use crate::{SandboxError, Spec, Support};

fn unsupported() -> SandboxError {
    SandboxError::Unsupported(format!("no sandbox backend on {}", std::env::consts::OS))
}

pub(crate) fn support() -> Support {
    Support {
        kind: "none",
        detail: format!("no sandbox backend on {}", std::env::consts::OS),
    }
}

pub(crate) fn command(spec: &Spec) -> Result<Command, SandboxError> {
    crate::prepare(spec)?;
    Err(unsupported())
}

pub(crate) fn spawn(
    spec: &Spec,
    _stdin: Stdio,
    _stdout: Stdio,
    _stderr: Stdio,
) -> Result<Process, SandboxError> {
    crate::prepare(spec)?;
    Err(unsupported())
}

/// Never constructed: `spawn` always fails on this platform.
pub(crate) struct Process(Infallible);

impl Process {
    pub(crate) fn id(&self) -> u32 {
        match self.0 {}
    }

    pub(crate) fn stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        match self.0 {}
    }

    pub(crate) fn stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        match self.0 {}
    }

    pub(crate) fn stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        match self.0 {}
    }

    pub(crate) fn wait(&mut self) -> std::io::Result<ExitStatus> {
        match self.0 {}
    }

    pub(crate) fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        match self.0 {}
    }

    pub(crate) fn kill(&mut self) -> std::io::Result<()> {
        match self.0 {}
    }
}
