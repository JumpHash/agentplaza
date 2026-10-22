//! Linux backend: Landlock for files and TCP ports, rlimits for the rest.
//!
//! The ruleset is built in the parent, which opens paths and allocates. The child
//! only makes the landlock_restrict_self syscall and setrlimit calls, then execs.

use std::io;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

use landlock::{
    ABI, Access, AccessFs, AccessNet, BitFlags, CompatLevel, Compatible, NetPort, PathBeneath,
    PathFd, Ruleset, RulesetAttr, RulesetCreated, RulesetCreatedAttr, RulesetStatus, Scope,
};

use crate::unix;
use crate::{Network, Prepared, SandboxError, Spec, Support};

pub(crate) use crate::unix::Process;

/// Read-and-execute paths every child gets, where they exist. `/proc` is not granted
/// as a whole: it would expose other processes' environments and memory (`/proc/PID/
/// environ`, `/proc/PID/mem`). Only the system-wide information files are listed.
const SYSTEM_READ: &[&str] = &[
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib64",
    "/etc",
    "/opt",
    "/sys",
    "/dev",
    "/proc/cpuinfo",
    "/proc/meminfo",
    "/proc/stat",
    "/proc/version",
    "/proc/uptime",
    "/proc/loadavg",
    "/proc/filesystems",
    "/proc/sys",
];

/// The temp directory: read and write.
const TEMP_DIR: &str = "/tmp";

/// The one device the child may write to.
const NULL_DEVICE: &str = "/dev/null";

/// The highest Landlock ABI the running kernel supports, or `None` when Landlock is
/// absent or disabled. Each probe asks for the ABI's rights with `HardRequirement`, so
/// the kernel refuses any right it does not know, and the first ABI that succeeds wins.
fn kernel_abi() -> Option<ABI> {
    const NEWEST_FIRST: [ABI; 9] = [
        ABI::V9,
        ABI::V8,
        ABI::V7,
        ABI::V6,
        ABI::V5,
        ABI::V4,
        ABI::V3,
        ABI::V2,
        ABI::V1,
    ];
    NEWEST_FIRST.into_iter().find(|&abi| {
        Ruleset::default()
            .set_compatibility(CompatLevel::HardRequirement)
            .handle_access(AccessFs::from_all(abi))
            .and_then(|ruleset| ruleset.create())
            .is_ok()
    })
}

pub(crate) fn support() -> Support {
    let Some(abi) = kernel_abi() else {
        return Support {
            kind: "none",
            detail: "Landlock is not available: needs Linux 5.13+ with Landlock enabled at boot"
                .to_owned(),
        };
    };
    let network = if abi >= ABI::V4 {
        "TCP connect and bind are denied unless a rule allows them. LoopbackPort allows \
         connect to that TCP port on any address, because Landlock filters ports, not addresses"
    } else {
        "NETWORK IS NOT CONFINED: this kernel's Landlock has no TCP rules (ABI 4 needs Linux 6.7)"
    };
    let scoping = if abi >= ABI::V6 {
        "signals and abstract unix sockets to processes outside the sandbox are blocked"
    } else {
        "signals to outside processes are not blocked (ABI 6 needs Linux 6.12)"
    };
    let links = if abi >= ABI::V2 {
        "hard links cannot pull files in from outside the grants"
    } else {
        "HARD LINKS ARE NOT CONFINED: a file outside the grants can be linked into them \
         (ABI 2 needs Linux 5.19)"
    };
    Support {
        kind: "landlock",
        detail: format!(
            "Landlock ABI {}: file reads, writes and exec limited to the granted paths \
             (system paths and a few /proc info files are read-only; /tmp is read and write; \
             /dev/null is writable; /proc/PID is not granted); {links}; {network}; UDP, raw \
             and other sockets are not confined; {scoping}; ptrace of same-user processes is \
             left to the kernel's Yama policy; cpu via RLIMIT_CPU; memory via RLIMIT_AS and \
             RLIMIT_DATA; processes via RLIMIT_NPROC (counted per user)",
            abi as i32
        ),
    }
}

pub(crate) fn command(spec: &Spec) -> Result<Command, SandboxError> {
    let prepared = crate::prepare(spec)?;
    if kernel_abi().is_none() {
        return Err(SandboxError::Unsupported(
            "Landlock is not available: needs Linux 5.13+ with Landlock enabled at boot".to_owned(),
        ));
    }
    let ruleset = build_ruleset(&prepared)?;

    let mut command = Command::new(&prepared.program);
    command.args(&prepared.args);
    command.env_clear();
    command.envs(
        prepared
            .env
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str())),
    );
    command.current_dir(&prepared.workdir);

    let limits = prepared.limits;
    let mut ruleset = Some(ruleset);
    // SAFETY: the closure runs in the forked child before exec. It makes the
    // landlock_restrict_self syscall, closes descriptors and calls setrlimit, and
    // allocates nothing on success. `take` runs on the child's copy of the closure, so
    // the command can be spawned again. The ruleset is enforced first: its descriptor
    // is consumed there, before the close pass runs.
    unsafe {
        command.pre_exec(move || {
            if let Some(created) = ruleset.take() {
                enforce(created)?;
            }
            unix::close_inherited_fds();
            unix::apply_rlimits(&limits)
        })
    };
    Ok(command)
}

pub(crate) fn spawn(
    spec: &Spec,
    stdin: Stdio,
    stdout: Stdio,
    stderr: Stdio,
) -> Result<Process, SandboxError> {
    unix::run(command(spec)?, stdin, stdout, stderr)
}

/// Applies the ruleset to the calling thread. Only a partial result is accepted:
/// best effort means a kernel that lacks some rights still gets the rest.
fn enforce(created: RulesetCreated) -> io::Result<()> {
    match created.restrict_self() {
        Ok(status) if status.ruleset != RulesetStatus::NotEnforced => Ok(()),
        Ok(_) => Err(io::Error::from_raw_os_error(libc::ENOSYS)),
        Err(err) => Err(io::Error::other(err)),
    }
}

fn build_ruleset(p: &Prepared) -> Result<RulesetCreated, SandboxError> {
    // BestEffort: rights the running kernel lacks are dropped, and support() reports them.
    let mut created = Ruleset::default()
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(AccessFs::from_all(ABI::V9))
        .and_then(|ruleset| ruleset.handle_access(AccessNet::from_all(ABI::V9)))
        .and_then(|ruleset| ruleset.scope(Scope::from_all(ABI::V9)))
        .and_then(|ruleset| ruleset.create())
        .map_err(landlock_error)?;

    let read = AccessFs::from_read(ABI::V9);
    let write = AccessFs::from_all(ABI::V9);

    for path in SYSTEM_READ.iter().map(Path::new) {
        if path.exists() {
            created = allow(created, path, read)?;
        }
    }
    created = allow(created, &p.program_real, read)?;
    for path in &p.read_only {
        created = allow(created, path, read)?;
    }
    created = allow(created, &p.workdir, write)?;
    for path in &p.read_write {
        created = allow(created, path, write)?;
    }
    let temp = Path::new(TEMP_DIR);
    if temp.exists() {
        created = allow(created, temp, write)?;
    }
    created = allow(created, Path::new(NULL_DEVICE), write)?;

    if let Network::LoopbackPort(port) = p.network {
        created = created
            .add_rule(NetPort::new(port, AccessNet::ConnectTcp))
            .map_err(landlock_error)?;
    }
    Ok(created)
}

/// Adds a rule granting `access` beneath `path`. Landlock keeps only the rights that
/// fit a file when `path` is a file.
fn allow(
    created: RulesetCreated,
    path: &Path,
    access: BitFlags<AccessFs>,
) -> Result<RulesetCreated, SandboxError> {
    let fd = PathFd::new(path)
        .map_err(|err| SandboxError::Invalid(format!("{}: {err}", path.display())))?;
    created
        .add_rule(PathBeneath::new(fd, access))
        .map_err(landlock_error)
}

fn landlock_error(err: impl std::fmt::Display) -> SandboxError {
    SandboxError::Io(io::Error::other(err.to_string()))
}
