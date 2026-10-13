//! What a local plaza needs, checked (see `super`).

use super::{Check, Mode, Need, State};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

const SERVICES: [&str; 3] = ["apid", "vsockd", "noded"];
const PORTS: [u16; 3] = [7700, 8899, 19080];
const POSTGRES: &str = "agentplaza-postgres";
const FORGEJO: &str = "agentplaza-forgejo";
const POSTGRES_CREATE: &str = "Create the container agentplaza-postgres (Postgres 17, data in the volume agentplaza-postgres, on 127.0.0.1:5434) and record its address in ~/.config/agentplaza/keys.env";
const FORGEJO_CREATE: &str = "Create the container agentplaza-forgejo (Forgejo 11, data in the volume agentplaza-forgejo, on 127.0.0.1:3300), its admin and API token, and record them in ~/.config/agentplaza/keys.env";
const WHP_QUERY: &str = "(Get-CimInstance Win32_OptionalFeature -Filter \"Name='HypervisorPlatform'\").InstallState";

/// Every requirement for `mode`, in the order the checklist shows them.
pub fn check_all(_mode: Mode) -> Vec<Check> {
    let dirs = search_dirs();
    let version = docker_version();
    let daemon = if version.is_some() { docker_daemon() } else { Daemon::NotAnswering };
    let docker_up = matches!(daemon, Daemon::Running(_));
    vec![
        services(&dirs),
        docker(version.as_deref()),
        docker_running(version.is_some(), &daemon),
        container_check(Need::Postgres, "Postgres", POSTGRES, docker_up, POSTGRES_CREATE),
        container_check(Need::Forgejo, "Forgejo", FORGEJO, docker_up, FORGEJO_CREATE),
        hypervisor(),
        ports(),
    ]
}

/// Builds a row. `fix` is kept only when the state is `Missing`.
fn check(
    need: Need,
    label: &'static str,
    state: State,
    detail: impl Into<String>,
    fix: Option<String>,
    required: bool,
) -> Check {
    let fix = if state == State::Missing { fix } else { None };
    Check { need, label, state, detail: detail.into(), fix, required }
}

/// Runs `program args` with no stdin; `None` if it cannot run at all.
fn probe(program: &str, args: &[&str]) -> Option<Output> {
    super::command(program).args(args).stdin(Stdio::null()).output().ok()
}

/// The first line of a command's output, trimmed.
fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).lines().next().unwrap_or("").trim().to_string()
}

/// The directories to search for the plaza's binaries: the one holding this app, then PATH.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .into_iter()
        .collect();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs
}

/// The first of `dirs` holding the executable `name` (`name.exe` on Windows).
fn locate(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let exe = apz_os::process::exe(name);
    dirs.iter().map(|dir| dir.join(&exe)).find(|path| path.is_file())
}

fn services(dirs: &[PathBuf]) -> Check {
    // Only apid ships for macOS x86_64: noded and vsockd are Apple silicon builds.
    let needed: &[&str] = if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        &SERVICES[..1]
    } else {
        &SERVICES
    };
    let found: Vec<(&str, Option<PathBuf>)> =
        needed.iter().map(|name| (*name, locate(*name, dirs))).collect();
    let missing: Vec<&str> =
        found.iter().filter(|(_, path)| path.is_none()).map(|(name, _)| *name).collect();
    if missing.is_empty() {
        let dir = found
            .first()
            .and_then(|(_, path)| path.as_ref())
            .and_then(|path| path.parent())
            .map(|dir| dir.display().to_string())
            .unwrap_or_default();
        check(Need::Services, "AgentPlaza services", State::Ok, dir, None, true)
    } else {
        check(
            Need::Services,
            "AgentPlaza services",
            State::Blocked,
            format!("missing: {}; reinstall AgentPlaza", missing.join(", ")),
            None,
            true,
        )
    }
}

fn docker_install_fix() -> &'static str {
    if cfg!(target_os = "linux") {
        "Install Docker Engine with your package manager (asks for your password), start it, and let your user run it"
    } else if cfg!(target_os = "macos") {
        "Install Docker Desktop (with Homebrew if you have it, else from docker.com), then open it"
    } else {
        "Install Docker Desktop with winget (Windows asks for permission), then open it"
    }
}

/// The installed `docker` version (its first line), or `None` if the command is absent.
fn docker_version() -> Option<String> {
    let out = probe("docker", &["--version"])?;
    out.status.success().then(|| first_line(&out.stdout))
}

fn docker(version: Option<&str>) -> Check {
    match version {
        Some(version) => check(Need::Docker, "Docker", State::Ok, version, None, true),
        None => check(
            Need::Docker,
            "Docker",
            State::Missing,
            "not installed",
            Some(docker_install_fix().to_string()),
            true,
        ),
    }
}

enum Daemon {
    Running(String),
    /// Linux: the user may not reach the daemon's socket yet (docker group).
    Denied,
    NotAnswering,
}

fn docker_daemon() -> Daemon {
    let Some(out) = probe("docker", &["info", "--format", "{{.ServerVersion}}"]) else {
        return Daemon::NotAnswering;
    };
    if out.status.success() {
        return Daemon::Running(first_line(&out.stdout));
    }
    let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
    if cfg!(target_os = "linux") && stderr.contains("permission denied") {
        Daemon::Denied
    } else {
        Daemon::NotAnswering
    }
}

fn docker_running(installed: bool, daemon: &Daemon) -> Check {
    if !installed {
        return check(Need::DockerRunning, "Docker running", State::Missing, "install Docker first", None, true);
    }
    match daemon {
        Daemon::Running(version) => check(
            Need::DockerRunning,
            "Docker running",
            State::Ok,
            format!("Docker {version} running"),
            None,
            true,
        ),
        Daemon::Denied => check(
            Need::DockerRunning,
            "Docker running",
            State::Blocked,
            "your user cannot use Docker yet: log out and back in (you were added to the docker group)",
            None,
            true,
        ),
        Daemon::NotAnswering => check(
            Need::DockerRunning,
            "Docker running",
            State::Missing,
            "Docker is installed but not answering",
            Some("Start Docker and wait until it answers".to_string()),
            true,
        ),
    }
}

enum Container {
    Running,
    Stopped,
    Absent,
}

fn container_state(name: &str) -> Container {
    match probe("docker", &["inspect", "-f", "{{.State.Running}}", name]) {
        Some(out) if out.status.success() => {
            if first_line(&out.stdout) == "true" {
                Container::Running
            } else {
                Container::Stopped
            }
        }
        _ => Container::Absent,
    }
}

fn container_check(need: Need, label: &'static str, name: &str, docker_up: bool, create: &str) -> Check {
    if !docker_up {
        return check(need, label, State::Missing, "needs Docker running", None, true);
    }
    match container_state(name) {
        Container::Running => {
            check(need, label, State::Ok, format!("container {name} running"), None, true)
        }
        Container::Stopped => check(
            need,
            label,
            State::Missing,
            format!("container {name} stopped"),
            Some(format!("Start the container {name}")),
            true,
        ),
        Container::Absent => check(
            need,
            label,
            State::Missing,
            format!("container {name} not found"),
            Some(create.to_string()),
            true,
        ),
    }
}

fn hypervisor() -> Check {
    let (state, detail) = hypervisor_state();
    check(Need::Hypervisor, "Hypervisor", state, detail, None, false)
}

fn hypervisor_state() -> (State, String) {
    if cfg!(target_os = "linux") {
        let kvm = std::fs::OpenOptions::new().read(true).write(true).open("/dev/kvm");
        return match kvm {
            Ok(_) => (State::Ok, "/dev/kvm usable".to_string()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => (
                State::Warning,
                "agents need /dev/kvm: turn on virtualization in the firmware".to_string(),
            ),
            Err(_) => (
                State::Warning,
                "agents need /dev/kvm: turn on virtualization in the firmware, and add yourself to the kvm group (sudo usermod -aG kvm $USER, then log in again)".to_string(),
            ),
        };
    }
    if cfg!(target_os = "macos") {
        return if cfg!(target_arch = "aarch64") {
            (State::Ok, "Hypervisor.framework".to_string())
        } else {
            (
                State::Warning,
                "Intel Macs run the hub and the app; agents run on nodes elsewhere".to_string(),
            )
        };
    }
    if cfg!(windows) {
        let enabled = probe("powershell", &["-NoProfile", "-Command", WHP_QUERY])
            .is_some_and(|out| out.status.success() && first_line(&out.stdout) == "1");
        return if enabled {
            (State::Ok, "Windows Hypervisor Platform on".to_string())
        } else {
            (
                State::Warning,
                "agents need the Windows Hypervisor Platform: in an administrator PowerShell, Enable-WindowsOptionalFeature -Online -FeatureName HypervisorPlatform -All, then restart".to_string(),
            )
        };
    }
    (State::Warning, "agents need a hypervisor: not checked on this OS".to_string())
}

/// The first port of `ports` another process holds on 127.0.0.1.
fn busy_port(ports: &[u16]) -> Option<u16> {
    ports.iter().copied().find(|port| TcpListener::bind(("127.0.0.1", *port)).is_err())
}

/// Whether AgentPlaza's own hub answers on 127.0.0.1:7700 (its overview names the agents): then
/// the plaza is already running and its ports are held by it, not by something in the way.
fn plaza_running() -> bool {
    use std::io::{Read, Write};
    use std::time::Duration;
    let Ok(mut conn) = std::net::TcpStream::connect_timeout(&([127, 0, 0, 1], 7700).into(), Duration::from_millis(500)) else {
        return false;
    };
    let _ = conn.set_read_timeout(Some(Duration::from_secs(2)));
    if conn.write_all(b"GET /v1/overview HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n").is_err() {
        return false;
    }
    let mut answer = Vec::new();
    let _ = conn.take(64 * 1024).read_to_end(&mut answer);
    let answer = String::from_utf8_lossy(&answer);
    answer.starts_with("HTTP/1.") && answer.contains(" 200 ") && answer.contains("\"agents\"")
}

fn ports() -> Check {
    if busy_port(&PORTS).is_some() && plaza_running() {
        return check(Need::Ports, "Ports", State::Ok, "AgentPlaza is already running here", None, true);
    }
    match busy_port(&PORTS) {
        Some(port) => check(
            Need::Ports,
            "Ports",
            State::Blocked,
            format!("port {port} is in use: stop what holds it"),
            None,
            true,
        ),
        None => check(Need::Ports, "Ports", State::Ok, "7700, 8899, 19080 free", None, true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-setup-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn locate_finds_the_binary_in_the_first_directory_holding_it() {
        let a = scratch("locate-a");
        let b = scratch("locate-b");
        let apid = b.join(apz_os::process::exe("apid"));
        std::fs::write(&apid, b"").unwrap();
        let dirs = vec![a.clone(), b.clone()];
        assert_eq!(locate("apid", &dirs), Some(apid));
        assert_eq!(locate("vsockd", &dirs), None);
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn services_are_blocked_when_a_binary_is_missing() {
        let check = services(&[]);
        assert_eq!(check.state, State::Blocked);
        assert!(check.detail.starts_with("missing: "), "{}", check.detail);
        assert!(check.detail.contains("apid"), "{}", check.detail);
        assert!(check.fix.is_none());
    }

    #[test]
    fn a_bound_port_counts_as_busy() {
        // (Not "free once released": tests run in parallel, and another may take the port first.)
        let held = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        assert_eq!(busy_port(&[port]), Some(port));
    }

    #[test]
    fn check_all_lists_every_need_in_order() {
        let needs: Vec<Need> = check_all(Mode::Local).iter().map(|c| c.need).collect();
        assert_eq!(
            needs,
            vec![
                Need::Services,
                Need::Docker,
                Need::DockerRunning,
                Need::Postgres,
                Need::Forgejo,
                Need::Hypervisor,
                Need::Ports,
            ]
        );
    }
}
