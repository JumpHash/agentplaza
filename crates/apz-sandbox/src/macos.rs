//! macOS backend: a Seatbelt profile applied by `/usr/bin/sandbox-exec`.
//!
//! The profile denies everything and then allows, by name: forking, exec of the
//! granted trees, reads of the system paths and the granted trees, writes to the
//! workdir, the read-write paths, `/tmp` and three devices, a fixed set of Mach
//! services, and TCP to one localhost port when asked. `stat()` is not confined:
//! names and sizes are visible, contents are not.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::unix;
use crate::{Limits, Network, Prepared, SandboxError, Spec, Support};

pub(crate) use crate::unix::Process;

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// Paths every child may read and execute. Canonical forms: `/etc` is `/private/etc`.
const SYSTEM_PATHS: &[&str] = &[
    "/usr",
    "/bin",
    "/sbin",
    "/System",
    "/Library",
    "/private/etc",
    "/private/var/db/timezone",
    "/dev",
    "/opt/homebrew",
    "/usr/local",
];

/// The temp directory the child may read and write: `/tmp`, canonically `/private/tmp`.
/// The user's `$TMPDIR` (under `/private/var/folders`) is not granted.
const TEMP_DIR: &str = "/private/tmp";

/// Devices the child may write to.
const DEVICES: &[&str] = &["/dev/null", "/dev/tty", "/dev/dtracehelper"];

/// Mach services a common runtime needs: logging, notify, and user/group/host lookups
/// through libinfo (which also resolves `localhost`). mDNSResponder (DNS) and cfprefsd
/// (preferences) are left out: a sandboxed process could otherwise reach the network
/// or the user's settings through those daemons.
const MACH_SERVICES: &[&str] = &[
    "com.apple.system.logger",
    "com.apple.system.notification_center",
    "com.apple.system.opendirectoryd.libinfo",
    "com.apple.system.opendirectoryd.membership",
    "com.apple.logd",
    "com.apple.logd.events",
];

pub(crate) fn support() -> Support {
    if Path::new(SANDBOX_EXEC).is_file() {
        Support {
            kind: "seatbelt",
            detail: "deny-by-default Seatbelt profile. Reads: system paths, workdir, \
                     read_only, read_write, /tmp and the program. Writes: workdir, \
                     read_write, /tmp, /dev/null, /dev/tty. Network: none, or TCP to \
                     localhost:PORT only (127.0.0.1 and ::1); no listening. Mach services: \
                     a fixed list (no DNS, no preferences); POSIX semaphores and shared \
                     memory allowed. Not confined: stat(), and the argv of other same-user \
                     processes (kern.procargs2 is not mediated), so keep secrets out of argv. \
                     Limits: cpu via RLIMIT_CPU, processes via RLIMIT_NPROC (counted per \
                     user). memory_bytes is refused: macOS rejects address-space rlimits."
                .to_owned(),
        }
    } else {
        Support {
            kind: "none",
            detail: format!("{SANDBOX_EXEC} is missing"),
        }
    }
}

pub(crate) fn command(spec: &Spec) -> Result<Command, SandboxError> {
    let prepared = crate::prepare(spec)?;
    command_for(&prepared)
}

fn command_for(p: &Prepared) -> Result<Command, SandboxError> {
    if p.limits.memory_bytes.is_some() {
        return Err(SandboxError::Unsupported(
            "memory_bytes: macOS rejects RLIMIT_AS and RLIMIT_DATA (EINVAL) and has no \
             unprivileged per-process memory cap"
                .to_owned(),
        ));
    }
    if !Path::new(SANDBOX_EXEC).is_file() {
        return Err(SandboxError::Unsupported(format!(
            "{SANDBOX_EXEC} is missing"
        )));
    }
    let profile = profile(p)?;

    let mut command = Command::new(SANDBOX_EXEC);
    // sandbox-exec applies the profile to itself, then execs `program`, so the
    // child keeps the same pid. `--` ends sandbox-exec's own options.
    command
        .arg("-p")
        .arg(profile)
        .arg("--")
        .arg(&p.program)
        .args(&p.args);
    command.env_clear();
    command.envs(
        p.env
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str())),
    );
    command.current_dir(&p.workdir);

    let limits: Limits = p.limits;
    // SAFETY: the closure runs in the forked child before exec. It only closes
    // descriptors and calls setrlimit (see `unix`), and allocates nothing.
    unsafe {
        command.pre_exec(move || {
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

/// The Seatbelt profile for `p`. Every path is canonical, because the kernel matches
/// the real path (`/var/...` is really `/private/var/...`).
fn profile(p: &Prepared) -> Result<String, SandboxError> {
    let system: Vec<&Path> = SYSTEM_PATHS.iter().map(Path::new).collect();
    let temp = Path::new(TEMP_DIR);
    let programs = [p.program.as_path(), p.program_real.as_path()];
    let devices: Vec<&Path> = DEVICES.iter().map(Path::new).collect();

    let mut readable = system;
    readable.push(p.workdir.as_path());
    readable.extend(p.read_only.iter().map(PathBuf::as_path));
    readable.extend(p.read_write.iter().map(PathBuf::as_path));
    readable.push(temp);

    let mut writable: Vec<&Path> = vec![p.workdir.as_path()];
    writable.extend(p.read_write.iter().map(PathBuf::as_path));
    writable.push(temp);

    let readable_filters = filters(&readable, &programs)?;
    let mut sb = String::from("(version 1)\n(deny default)\n");
    sb.push_str("(allow process-fork)\n");
    sb.push_str(&format!("(allow process-exec{readable_filters})\n"));
    sb.push_str("(allow signal (target same-sandbox))\n");
    sb.push_str("(allow sysctl-read)\n");
    sb.push_str("(allow system-socket)\n");
    sb.push_str("(allow file-read-metadata)\n");
    sb.push_str(&format!(
        "(allow file-read* (literal \"/\"){readable_filters})\n"
    ));
    sb.push_str(&format!("(allow file-map-executable{readable_filters})\n"));
    sb.push_str(&format!(
        "(allow file-write*{})\n",
        filters(&writable, &devices)?
    ));
    sb.push_str("(allow mach-lookup");
    for service in MACH_SERVICES {
        sb.push_str(&format!(" (global-name \"{service}\")"));
    }
    sb.push_str(")\n");
    sb.push_str("(allow ipc-posix-sem)\n(allow ipc-posix-shm)\n");
    if let Network::LoopbackPort(port) = p.network {
        sb.push_str(&format!(
            "(allow network-outbound (remote ip \"localhost:{port}\"))\n"
        ));
    }
    Ok(sb)
}

/// ` (subpath "…") … (literal "…")` for the given paths.
fn filters(subpaths: &[&Path], literals: &[&Path]) -> Result<String, SandboxError> {
    let mut out = String::new();
    for path in subpaths {
        out.push_str(&format!(" (subpath {})", quote(path)?));
    }
    for path in literals {
        out.push_str(&format!(" (literal {})", quote(path)?));
    }
    Ok(out)
}

/// A Seatbelt string literal for `path`, with `"` and `\` escaped. Non-UTF-8 paths
/// and control characters are refused: a lossy conversion would grant a different
/// path than the one asked for.
fn quote(path: &Path) -> Result<String, SandboxError> {
    let text = path
        .to_str()
        .ok_or_else(|| SandboxError::Invalid(format!("path is not UTF-8: {}", path.display())))?;
    if text.chars().any(char::is_control) {
        return Err(SandboxError::Invalid(format!(
            "path has a control character: {:?}",
            text
        )));
    }
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        if ch == '"' || ch == '\\' {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepared(network: Network, read_only: Vec<PathBuf>, read_write: Vec<PathBuf>) -> Prepared {
        Prepared {
            program: PathBuf::from("/usr/bin/true"),
            program_real: PathBuf::from("/usr/bin/true"),
            args: vec![],
            env: vec![],
            workdir: PathBuf::from("/private/tmp/apz-sbx-unit/work"),
            read_only,
            read_write,
            network,
            limits: Limits::default(),
        }
    }

    #[test]
    fn quote_escapes_quotes_and_backslashes() {
        assert_eq!(quote(Path::new(r#"/a"b\c"#)).unwrap(), r#""/a\"b\\c""#);
        assert_eq!(quote(Path::new("/a(b);c")).unwrap(), "\"/a(b);c\"");
        assert!(quote(Path::new("/a\nb")).is_err());
    }

    #[test]
    fn profile_grants_only_listed_paths_and_temp() {
        let p = prepared(
            Network::None,
            vec![PathBuf::from("/private/var/ro")],
            vec![PathBuf::from("/private/var/rw")],
        );
        let sb = profile(&p).unwrap();
        assert!(sb.starts_with("(version 1)\n(deny default)\n"));
        assert!(sb.contains("(allow file-write* (subpath \"/private/tmp/apz-sbx-unit/work\")"));
        assert!(sb.contains("(subpath \"/private/var/rw\")"));
        assert!(sb.contains("(subpath \"/private/tmp\")"));
        assert!(sb.contains("(literal \"/dev/null\")"));
        assert!(sb.contains("(subpath \"/private/var/ro\")"));
        assert!(!sb.contains("file-write* (subpath \"/private/var/ro\")"));
        assert!(!sb.contains("network-outbound"));
    }

    #[test]
    fn loopback_profile_allows_only_that_port() {
        let sb = profile(&prepared(Network::LoopbackPort(4242), vec![], vec![])).unwrap();
        assert!(sb.contains("(allow network-outbound (remote ip \"localhost:4242\"))"));
        assert_eq!(sb.matches("network-outbound").count(), 1);
    }

    #[test]
    fn prepare_canonicalizes_tmp_paths() {
        let dir = PathBuf::from(format!("/tmp/apz-sbx-unit-{}", std::process::id()));
        let spec = Spec {
            program: PathBuf::from("/bin/sh"),
            args: vec![],
            env: vec![],
            workdir: dir.join("work"),
            read_only: vec![],
            read_write: vec![dir.join("cache")],
            network: Network::None,
            limits: Limits::default(),
        };
        let p = crate::prepare(&spec).expect("prepare");
        assert!(
            p.workdir.starts_with("/private/tmp"),
            "{}",
            p.workdir.display()
        );
        assert!(p.read_write[0].starts_with("/private/tmp"));
        let sb = profile(&p).unwrap();
        assert!(!sb.contains("\"/tmp/"), "profile must not name /tmp");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn memory_limit_is_refused_on_macos() {
        let spec = Spec {
            program: PathBuf::from("/bin/sh"),
            args: vec![],
            env: vec![],
            workdir: PathBuf::from(format!("/tmp/apz-sbx-mem-{}", std::process::id())),
            read_only: vec![],
            read_write: vec![],
            network: Network::None,
            limits: Limits {
                memory_bytes: Some(1 << 30),
                ..Limits::default()
            },
        };
        let result = command(&spec);
        assert!(matches!(result, Err(SandboxError::Unsupported(_))));
        let _ = std::fs::remove_dir_all(&spec.workdir);
    }
}
