//! Runs a child process confined by the operating system's own sandbox, so the
//! plaza needs no Docker. MCP stdio servers and ML trainers run through [`spawn`]
//! (or [`command`] on Unix).
//!
//! - macOS: a deny-by-default Seatbelt profile applied by `sandbox-exec`.
//! - Linux: Landlock rules and rlimits, applied in the child right before `exec`.
//! - Windows: an AppContainer with no capabilities, inside a job object.
//! - Other platforms: [`support`] reports `"none"` and spawning fails.
//!
//! [`support`] says what the running system actually enforces. Read its `detail`:
//! the backends differ, and some limits are weaker than their names suggest.

use std::error::Error;
use std::fmt;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as backend;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as backend;

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod unix;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as backend;

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod other;
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
use other as backend;

/// What to run and what it may touch.
#[derive(Debug, Clone)]
pub struct Spec {
    /// The program: an absolute path, or a bare name looked up in `env`'s `PATH`.
    pub program: PathBuf,
    pub args: Vec<String>,
    /// The child's whole environment: nothing is inherited except what is listed here.
    pub env: Vec<(String, String)>,
    /// Created if missing; the child's cwd; readable and writable.
    pub workdir: PathBuf,
    /// More paths the child may read (a runtime: node, python, a uv cache).
    pub read_only: Vec<PathBuf>,
    /// More paths the child may read and write: directories (caches; created if missing), or existing files and device
    /// nodes (a GPU's), granted as they are.
    pub read_write: Vec<PathBuf>,
    pub network: Network,
    pub limits: Limits,
}

/// The network the child may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    /// No network access.
    None,
    /// TCP to localhost on this port only: the hub's egress proxy, or a loose trainer job's hub. Each backend's exact
    /// reach is described in [`support`]'s `detail`. Windows refuses this variant, and Linux refuses it below Landlock
    /// ABI 4 (Linux 6.7), where the kernel cannot filter TCP ports.
    LoopbackPort(u16),
}

/// Resource limits. `None` means no limit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Limits {
    /// Memory cap in bytes: RLIMIT_AS and RLIMIT_DATA on Linux, a per-process job limit
    /// on Windows. macOS refuses it with [`SandboxError::Unsupported`]: the kernel rejects
    /// address-space rlimits, and offers no unprivileged per-process memory cap.
    pub memory_bytes: Option<u64>,
    /// Process cap. On Unix this is RLIMIT_NPROC, which counts every process of the
    /// user, not just the sandbox's: a value below the user's current count stops
    /// the child from forking at all. On Windows it is a per-job active-process limit.
    pub processes: Option<u32>,
    /// CPU time in seconds (RLIMIT_CPU, or the job's per-process user-time limit).
    pub cpu_seconds: Option<u64>,
}

/// What this platform enforces, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Support {
    /// `"seatbelt"`, `"landlock"`, `"appcontainer"` or `"none"`.
    pub kind: &'static str,
    /// What is enforced, what is not, and any kernel-dependent gaps.
    pub detail: String,
}

/// Why a sandbox could not be built or started.
#[derive(Debug)]
pub enum SandboxError {
    /// This platform or kernel cannot enforce what the spec asks for.
    Unsupported(String),
    /// An OS call failed.
    Io(io::Error),
    /// The spec is malformed: a relative path, a bad environment name, and so on.
    Invalid(String),
}

impl fmt::Display for SandboxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SandboxError::Unsupported(why) => write!(f, "sandbox unsupported: {why}"),
            SandboxError::Io(err) => write!(f, "sandbox I/O error: {err}"),
            SandboxError::Invalid(why) => write!(f, "invalid sandbox spec: {why}"),
        }
    }
}

impl Error for SandboxError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            SandboxError::Io(err) => Some(err),
            SandboxError::Unsupported(_) | SandboxError::Invalid(_) => None,
        }
    }
}

impl From<io::Error> for SandboxError {
    fn from(err: io::Error) -> Self {
        SandboxError::Io(err)
    }
}

/// Reports what this platform enforces.
pub fn support() -> Support {
    backend::support()
}

/// A [`Command`] that runs `spec` confined. The caller sets stdio and spawns it.
/// On Windows this returns [`SandboxError::Unsupported`]: use [`spawn`] instead.
pub fn command(spec: &Spec) -> Result<Command, SandboxError> {
    backend::command(spec)
}

/// Starts `spec` confined, with the given stdio. On Windows the three streams are
/// always piped, whatever `Stdio` is passed (see [`Sandboxed`]).
pub fn spawn(
    spec: &Spec,
    stdin: Stdio,
    stdout: Stdio,
    stderr: Stdio,
) -> Result<Sandboxed, SandboxError> {
    backend::spawn(spec, stdin, stdout, stderr).map(|inner| Sandboxed { inner })
}

/// A running confined process.
///
/// Each stream handle is handed out once, by `stdin`, `stdout` or `stderr`, and only
/// when the matching [`Stdio`] was piped. On Windows all three are always piped
/// (the `Stdio` arguments cannot be inspected there), so drain stdout and stderr or
/// the child can block.
///
/// Dropping it does not stop the child on Unix. On Windows it kills the job, and so
/// every process the sandbox started.
pub struct Sandboxed {
    inner: backend::Process,
}

impl Sandboxed {
    pub fn id(&self) -> u32 {
        self.inner.id()
    }

    pub fn stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        self.inner.stdin()
    }

    pub fn stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        self.inner.stdout()
    }

    pub fn stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        self.inner.stderr()
    }

    /// Blocks until the child exits.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.inner.wait()
    }

    /// Returns the exit status if the child has exited, without blocking.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.inner.try_wait()
    }

    /// Kills the child and what it started: on Unix its whole process group (SIGKILL; the
    /// child leads its own group), on Windows the whole job.
    pub fn kill(&mut self) -> io::Result<()> {
        self.inner.kill()
    }
}

/// A [`Spec`] checked and resolved: absolute paths, symlinks followed, and the
/// directories it needs created.
pub(crate) struct Prepared {
    /// The program as the caller named it (argv[0] and the exec target).
    pub(crate) program: PathBuf,
    /// The program with symlinks resolved; the file a sandbox must let the child run.
    pub(crate) program_real: PathBuf,
    pub(crate) args: Vec<String>,
    pub(crate) env: Vec<(String, String)>,
    pub(crate) workdir: PathBuf,
    pub(crate) read_only: Vec<PathBuf>,
    pub(crate) read_write: Vec<PathBuf>,
    pub(crate) network: Network,
    pub(crate) limits: Limits,
}

pub(crate) fn prepare(spec: &Spec) -> Result<Prepared, SandboxError> {
    for (key, value) in &spec.env {
        if key.is_empty() || key.contains('=') || key.contains('\0') {
            return Err(invalid(format!("bad environment name {key:?}")));
        }
        if value.contains('\0') {
            return Err(invalid(format!("environment value of {key} contains NUL")));
        }
    }
    if spec.args.iter().any(|arg| arg.contains('\0')) {
        return Err(invalid("an argument contains NUL"));
    }
    if spec.network == Network::LoopbackPort(0) {
        return Err(invalid("LoopbackPort(0) is not a port"));
    }

    let program = resolve_program(&spec.program, &spec.env)?;
    let program_real = real_path(&program)?;
    let workdir = writable_dir(&spec.workdir)?;
    let read_only = spec
        .read_only
        .iter()
        .map(|path| real_path(path))
        .collect::<Result<Vec<_>, _>>()?;
    let read_write = spec
        .read_write
        .iter()
        .map(|path| writable_path(path))
        .collect::<Result<Vec<_>, _>>()?;

    for granted in [&workdir].into_iter().chain(&read_write).chain(&read_only) {
        if granted.parent().is_none() {
            return Err(invalid(format!(
                "refusing to grant the filesystem root: {}",
                granted.display()
            )));
        }
    }

    Ok(Prepared {
        program,
        program_real,
        args: spec.args.clone(),
        env: spec.env.clone(),
        workdir,
        read_only,
        read_write,
        network: spec.network,
        limits: spec.limits,
    })
}

fn invalid(why: impl Into<String>) -> SandboxError {
    SandboxError::Invalid(why.into())
}

/// An absolute path that exists, with symlinks followed.
fn real_path(path: &Path) -> Result<PathBuf, SandboxError> {
    if !path.is_absolute() {
        return Err(invalid(format!(
            "path must be absolute: {}",
            path.display()
        )));
    }
    let real =
        std::fs::canonicalize(path).map_err(|err| invalid(format!("{}: {err}", path.display())))?;
    Ok(strip_verbatim(real))
}

/// An absolute directory, created if missing, with symlinks followed.
fn writable_dir(path: &Path) -> Result<PathBuf, SandboxError> {
    if !path.is_absolute() {
        return Err(invalid(format!(
            "path must be absolute: {}",
            path.display()
        )));
    }
    std::fs::create_dir_all(path)?;
    real_path(path)
}

/// A path the child may write: an existing file or device node (a GPU's `/dev/nvidia0`) as it is, anything else a
/// directory, created if missing.
fn writable_path(path: &Path) -> Result<PathBuf, SandboxError> {
    match std::fs::metadata(path) {
        Ok(meta) if !meta.is_dir() => real_path(path),
        _ => writable_dir(path),
    }
}

/// Finds the program: an absolute path is used as given; a bare name is searched in
/// the absolute entries of the spec's own `PATH`. Relative entries are skipped, so a
/// planted `./name` cannot shadow a system binary.
fn resolve_program(program: &Path, env: &[(String, String)]) -> Result<PathBuf, SandboxError> {
    if program.is_absolute() {
        return Ok(program.to_path_buf());
    }
    let mut parts = program.components();
    let bare = match (parts.next(), parts.next()) {
        (Some(Component::Normal(name)), None) => name.to_owned(),
        _ => {
            return Err(invalid(format!(
                "program must be absolute or a bare name: {}",
                program.display()
            )));
        }
    };
    let Some((_, path_var)) = env.iter().find(|(key, _)| key == "PATH") else {
        return Err(invalid(format!(
            "{} is relative and the spec's env has no PATH",
            program.display()
        )));
    };
    for dir in std::env::split_paths(path_var).filter(|dir| dir.is_absolute()) {
        let mut candidates = vec![dir.join(&bare)];
        if cfg!(windows) && Path::new(&bare).extension().is_none() {
            candidates.push(dir.join(format!("{}.exe", bare.to_string_lossy())));
        }
        if let Some(found) = candidates.into_iter().find(|candidate| candidate.is_file()) {
            return Ok(found);
        }
    }
    Err(invalid(format!(
        "{} not found on the spec's PATH",
        program.display()
    )))
}

/// Drops the `\\?\` prefix that `canonicalize` adds on Windows when the rest is a
/// plain drive path, because the ACL and process APIs want the ordinary form.
#[cfg(windows)]
fn strip_verbatim(path: PathBuf) -> PathBuf {
    if let Some(rest) = path.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        let bytes = rest.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && bytes[2] == b'\\'
        {
            return PathBuf::from(rest);
        }
    }
    path
}

#[cfg(not(windows))]
fn strip_verbatim(path: PathBuf) -> PathBuf {
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_program_resolves_on_spec_path_only_when_absolute_entry() {
        let dir = std::env::temp_dir().join(format!("apz-sbx-resolve-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("apz-sbx-probe-bin");
        std::fs::write(&bin, b"#!/bin/sh\n").unwrap();
        // PATH is joined with the host's separator: `:` on Unix, `;` on Windows.
        let path = std::env::join_paths([Path::new("relative-dir"), dir.as_path()]).unwrap();
        let env = vec![("PATH".to_owned(), path.to_string_lossy().into_owned())];
        let found = resolve_program(Path::new("apz-sbx-probe-bin"), &env).unwrap();
        assert_eq!(found, bin);
        assert!(resolve_program(Path::new("apz-sbx-missing"), &env).is_err());
        assert!(resolve_program(Path::new("sub/prog"), &env).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_relative_paths_and_bad_env() {
        let spec = Spec {
            program: PathBuf::from("/bin/sh"),
            args: vec![],
            env: vec![("A=B".to_owned(), "x".to_owned())],
            workdir: PathBuf::from("relative"),
            read_only: vec![],
            read_write: vec![],
            network: Network::None,
            limits: Limits::default(),
        };
        assert!(matches!(prepare(&spec), Err(SandboxError::Invalid(_))));
    }
}
