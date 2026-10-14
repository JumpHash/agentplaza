//! What a local plaza needs, checked (see `lib.rs`).

use crate::provision::{GARAGE, GARAGE_KEYS, keys_set};
use crate::runner::Output;
use crate::{Check, Env, Mode, Need, State};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

const SERVICES: [&str; 3] = ["apid", "vsockd", "noded"];
const PORTS: [u16; 3] = [7700, 8899, 19080];
const POSTGRES: &str = "agentplaza-postgres";
const FORGEJO: &str = "agentplaza-forgejo";
const POSTGRES_CREATE: &str = "Create the container agentplaza-postgres (Postgres 17, data in the volume agentplaza-postgres, on 127.0.0.1:5434) and record its address in ~/.config/agentplaza/keys.env";
const FORGEJO_CREATE: &str = "Create the container agentplaza-forgejo (Forgejo 11, data in the volume agentplaza-forgejo, on 127.0.0.1:3300), its admin and API token, and record them in ~/.config/agentplaza/keys.env";
const GARAGE_CREATE: &str = "Create the container agentplaza-garage (Garage 1.0, the S3 object store, on 127.0.0.1:3900; data in the volumes agentplaza-garage-meta and agentplaza-garage-data), the bucket agentplaza and a key for it, and record the endpoint and the key in ~/.config/agentplaza/keys.env";
const WHP_QUERY: &str =
    "(Get-CimInstance Win32_OptionalFeature -Filter \"Name='HypervisorPlatform'\").InstallState";

/// Every requirement for `mode` on this machine, in the order the checklist shows them.
pub fn check_all(_mode: Mode) -> Vec<Check> {
    let env = Env::system();
    check_with(&env, &search_dirs(), hypervisor(&env), ports())
}

/// The rows for `env`, with the plaza's binaries searched for in `dirs`. `hypervisor` and `ports`
/// are this machine's own rows, passed in: a test does not probe the machine it runs on.
pub fn check_with(env: &Env, dirs: &[PathBuf], hypervisor: Check, ports: Check) -> Vec<Check> {
    let version = docker_version(env);
    let daemon = if version.is_some() {
        docker_daemon(env)
    } else {
        Daemon::NotAnswering
    };
    let docker_up = matches!(daemon, Daemon::Running(_));
    vec![
        services(dirs),
        docker(version.as_deref()),
        docker_running(version.is_some(), &daemon),
        container_check(
            env,
            Need::Postgres,
            "Postgres",
            POSTGRES,
            docker_up,
            POSTGRES_CREATE,
        ),
        container_check(
            env,
            Need::Forgejo,
            "Forgejo",
            FORGEJO,
            docker_up,
            FORGEJO_CREATE,
        ),
        garage(env, docker_up),
        hypervisor,
        ports,
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
    Check {
        need,
        label,
        state,
        detail: detail.into(),
        fix,
        required,
    }
}

/// Runs `program args` through the runner; `None` if it cannot run at all.
fn probe(env: &Env, program: &str, args: &[&str]) -> Option<Output> {
    env.runner.output(program, args).ok()
}

/// The first line of a command's output, trimmed.
fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
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
    dirs.iter()
        .map(|dir| dir.join(&exe))
        .find(|path| path.is_file())
}

fn services(dirs: &[PathBuf]) -> Check {
    // Only apid ships for macOS x86_64: noded and vsockd are Apple silicon builds.
    let needed: &[&str] = if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        &SERVICES[..1]
    } else {
        &SERVICES
    };
    let found: Vec<(&str, Option<PathBuf>)> = needed
        .iter()
        .map(|name| (*name, locate(name, dirs)))
        .collect();
    let missing: Vec<&str> = found
        .iter()
        .filter(|(_, path)| path.is_none())
        .map(|(name, _)| *name)
        .collect();
    if missing.is_empty() {
        let dir = found
            .first()
            .and_then(|(_, path)| path.as_ref())
            .and_then(|path| path.parent())
            .map(|dir| dir.display().to_string())
            .unwrap_or_default();
        check(
            Need::Services,
            "AgentPlaza services",
            State::Ok,
            dir,
            None,
            true,
        )
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
fn docker_version(env: &Env) -> Option<String> {
    let out = probe(env, "docker", &["--version"])?;
    out.success.then(|| first_line(&out.stdout))
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

fn docker_daemon(env: &Env) -> Daemon {
    let Some(out) = probe(env, "docker", &["info", "--format", "{{.ServerVersion}}"]) else {
        return Daemon::NotAnswering;
    };
    if out.success {
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
        return check(
            Need::DockerRunning,
            "Docker running",
            State::Missing,
            "install Docker first",
            None,
            true,
        );
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

fn container_state(env: &Env, name: &str) -> Container {
    match probe(
        env,
        "docker",
        &["inspect", "-f", "{{.State.Running}}", name],
    ) {
        Some(out) if out.success => {
            if first_line(&out.stdout) == "true" {
                Container::Running
            } else {
                Container::Stopped
            }
        }
        _ => Container::Absent,
    }
}

fn container_check(
    env: &Env,
    need: Need,
    label: &'static str,
    name: &str,
    docker_up: bool,
    create: &str,
) -> Check {
    if !docker_up {
        return check(
            need,
            label,
            State::Missing,
            "needs Docker running",
            None,
            true,
        );
    }
    match container_state(env, name) {
        Container::Running => check(
            need,
            label,
            State::Ok,
            format!("container {name} running"),
            None,
            true,
        ),
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

/// Garage (optional: apid keeps the library's files locally without it). Its container, and its
/// address and keys in keys.env, which is where apid reads them.
fn garage(env: &Env, docker_up: bool) -> Check {
    let (need, label) = (Need::Garage, "Garage");
    if !docker_up {
        return check(
            need,
            label,
            State::Missing,
            "needs Docker running",
            None,
            false,
        );
    }
    let keys = keys_set(env, &GARAGE_KEYS);
    match (container_state(env, GARAGE), keys) {
        (Container::Absent, _) => check(
            need,
            label,
            State::Missing,
            format!("container {GARAGE} not found"),
            Some(GARAGE_CREATE.to_string()),
            false,
        ),
        (_, false) => check(
            need,
            label,
            State::Blocked,
            format!(
                "container {GARAGE} exists but its keys are not in keys.env: remove the container \
                 (docker rm -f {GARAGE}) and set it up again"
            ),
            None,
            false,
        ),
        (Container::Running, true) => check(
            need,
            label,
            State::Ok,
            format!("container {GARAGE} running, keys in keys.env"),
            None,
            false,
        ),
        (Container::Stopped, true) => check(
            need,
            label,
            State::Missing,
            format!("container {GARAGE} stopped"),
            Some(format!("Start the container {GARAGE}")),
            false,
        ),
    }
}

fn hypervisor(env: &Env) -> Check {
    let (state, detail) = hypervisor_state(env);
    check(Need::Hypervisor, "Hypervisor", state, detail, None, false)
}

fn hypervisor_state(env: &Env) -> (State, String) {
    if cfg!(target_os = "linux") {
        let kvm = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/kvm");
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
    if cfg!(all(windows, target_arch = "aarch64")) {
        return (
            State::Warning,
            "agents on Windows arm64 aren't supported yet: agents run on nodes elsewhere for now; the arm64 VM node is in development".to_string(),
        );
    }
    if cfg!(windows) {
        let enabled = probe(env, "powershell", &["-NoProfile", "-Command", WHP_QUERY])
            .is_some_and(|out| out.success && first_line(&out.stdout) == "1");
        return if enabled {
            (State::Ok, "Windows Hypervisor Platform on".to_string())
        } else {
            (
                State::Warning,
                "agents need the Windows Hypervisor Platform: in an administrator PowerShell, Enable-WindowsOptionalFeature -Online -FeatureName HypervisorPlatform -All, then restart".to_string(),
            )
        };
    }
    (
        State::Warning,
        "agents need a hypervisor: not checked on this OS".to_string(),
    )
}

/// The first port of `ports` another process holds on 127.0.0.1.
fn busy_port(ports: &[u16]) -> Option<u16> {
    ports
        .iter()
        .copied()
        .find(|port| TcpListener::bind(("127.0.0.1", *port)).is_err())
}

/// Whether AgentPlaza's own hub answers on 127.0.0.1:7700 (its overview names the agents): then
/// the plaza is already running and its ports are held by it, not by something in the way.
fn plaza_running() -> bool {
    use std::io::{Read, Write};
    use std::time::Duration;
    let Ok(mut conn) = std::net::TcpStream::connect_timeout(
        &([127, 0, 0, 1], 7700).into(),
        Duration::from_millis(500),
    ) else {
        return false;
    };
    let _ = conn.set_read_timeout(Some(Duration::from_secs(2)));
    if conn
        .write_all(b"GET /v1/overview HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut answer = Vec::new();
    let _ = conn.take(64 * 1024).read_to_end(&mut answer);
    let answer = String::from_utf8_lossy(&answer);
    answer.starts_with("HTTP/1.") && answer.contains(" 200 ") && answer.contains("\"agents\"")
}

fn ports() -> Check {
    if busy_port(&PORTS).is_some() && plaza_running() {
        return check(
            Need::Ports,
            "Ports",
            State::Ok,
            "AgentPlaza is already running here",
            None,
            true,
        );
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
        None => check(
            Need::Ports,
            "Ports",
            State::Ok,
            "7700, 8899, 19080 free",
            None,
            true,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::mock::{Mock, scratch};
    use std::fs;

    /// Hypervisor and ports rows for the tests: fixed, so no test probes this machine.
    fn fixed_rows() -> (Check, Check) {
        (
            check(
                Need::Hypervisor,
                "Hypervisor",
                State::Warning,
                "fixed for the test",
                None,
                false,
            ),
            check(
                Need::Ports,
                "Ports",
                State::Ok,
                "fixed for the test",
                None,
                true,
            ),
        )
    }

    /// Runs the checks against `runner` with keys.env in `dir` (`keys` is its text, if any).
    fn rows(runner: &Mock, dir: &Path, keys: Option<&str>) -> Vec<Check> {
        if let Some(keys) = keys {
            fs::write(dir.join("keys.env"), keys).unwrap();
        }
        let env = Env {
            runner,
            keys: Some(dir.join("keys.env")),
            user: Some("tester".into()),
        };
        let (hypervisor, ports) = fixed_rows();
        check_with(&env, &[], hypervisor, ports)
    }

    fn row(checks: &[Check], need: Need) -> &Check {
        checks.iter().find(|c| c.need == need).unwrap()
    }

    /// A Docker that is installed and answers, with `inspect` answering `inspect`.
    fn docker_up(inspect: Option<Output>) -> Mock {
        Mock::new(move |_, args| match args.first() {
            Some(&"--version") => Output::ok("Docker version 27.1.1, build abc\n"),
            Some(&"info") => Output::ok("27.1.1\n"),
            Some(&"inspect") => inspect
                .clone()
                .unwrap_or_else(|| Output::failed("No such object")),
            _ => Output::ok(""),
        })
    }

    const ALL_GARAGE_KEYS: &str = "APZ_S3_ENDPOINT=http://127.0.0.1:3900\nAPZ_S3_BUCKET=agentplaza\nAPZ_S3_REGION=garage\nAPZ_S3_ACCESS_KEY=GK1\nAPZ_S3_SECRET_KEY=s1\n";

    #[test]
    fn a_bound_port_counts_as_busy() {
        // (Not "free once released": tests run in parallel, and another may take the port first.)
        let held = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        assert_eq!(busy_port(&[port]), Some(port));
    }

    #[test]
    fn locate_finds_the_binary_in_the_first_directory_holding_it() {
        let a = scratch("locate-a");
        let b = scratch("locate-b");
        let apid = b.join(apz_os::process::exe("apid"));
        fs::write(&apid, b"").unwrap();
        let dirs = vec![a.clone(), b.clone()];
        assert_eq!(locate("apid", &dirs), Some(apid));
        assert_eq!(locate("vsockd", &dirs), None);
        let _ = fs::remove_dir_all(&a);
        let _ = fs::remove_dir_all(&b);
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
    fn check_with_lists_every_need_in_order() {
        let dir = scratch("checks-order");
        let needs: Vec<Need> = rows(&docker_up(None), &dir, None)
            .iter()
            .map(|c| c.need)
            .collect();
        assert_eq!(
            needs,
            vec![
                Need::Services,
                Need::Docker,
                Need::DockerRunning,
                Need::Postgres,
                Need::Forgejo,
                Need::Garage,
                Need::Hypervisor,
                Need::Ports,
            ]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_docker_the_install_comes_first_and_the_containers_wait_for_it() {
        let dir = scratch("checks-nodocker");
        let mock = Mock::new(|_, _| Output::failed("not found"));
        let checks = rows(&mock, &dir, None);
        assert_eq!(row(&checks, Need::Docker).state, State::Missing);
        assert!(row(&checks, Need::Docker).fix.is_some());
        assert_eq!(
            row(&checks, Need::DockerRunning).detail,
            "install Docker first"
        );
        for need in [Need::Postgres, Need::Forgejo, Need::Garage] {
            let check = row(&checks, need);
            assert_eq!(check.detail, "needs Docker running", "{need:?}");
            assert!(
                check.fix.is_none(),
                "{need:?}: nothing to run until Docker answers"
            );
        }
        assert_eq!(
            mock.calls(),
            vec!["docker --version"],
            "no daemon or container probe without Docker"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn garage_is_ok_when_its_container_runs_and_its_keys_are_in_keys_env() {
        let dir = scratch("checks-garage-ok");
        let mock = docker_up(Some(Output::ok("true\n")));
        let checks = rows(&mock, &dir, Some(ALL_GARAGE_KEYS));
        let garage = row(&checks, Need::Garage);
        assert_eq!(garage.state, State::Ok);
        assert!(!garage.required, "apid keeps files locally without Garage");
        assert!(garage.fix.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_garage_container_is_missing_with_its_fix() {
        let dir = scratch("checks-garage-absent");
        let checks = rows(&docker_up(None), &dir, None);
        let garage = row(&checks, Need::Garage);
        assert_eq!(garage.state, State::Missing);
        assert!(
            garage.detail.contains("agentplaza-garage not found"),
            "{}",
            garage.detail
        );
        assert!(garage.fix.as_deref().unwrap().contains("agentplaza-garage"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stopped_garage_is_started_and_a_running_one_without_its_keys_is_blocked() {
        let dir = scratch("checks-garage-stopped");
        let stopped = rows(
            &docker_up(Some(Output::ok("false\n"))),
            &dir,
            Some(ALL_GARAGE_KEYS),
        );
        assert_eq!(row(&stopped, Need::Garage).state, State::Missing);
        assert_eq!(
            row(&stopped, Need::Garage).fix.as_deref(),
            Some("Start the container agentplaza-garage")
        );

        let running_no_keys = rows(
            &docker_up(Some(Output::ok("true\n"))),
            &dir,
            Some("OTHER=1\n"),
        );
        let garage = row(&running_no_keys, Need::Garage);
        assert_eq!(garage.state, State::Blocked);
        assert!(
            garage.detail.contains("docker rm -f agentplaza-garage"),
            "{}",
            garage.detail
        );
        assert!(garage.fix.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn postgres_and_forgejo_report_their_containers() {
        let dir = scratch("checks-containers");
        let mock = docker_up(Some(Output::ok("true\n")));
        let checks = rows(&mock, &dir, None);
        assert_eq!(row(&checks, Need::Postgres).state, State::Ok);
        assert_eq!(row(&checks, Need::Forgejo).state, State::Ok);
        let calls = mock.calls();
        assert!(
            calls.contains(&"docker inspect -f {{.State.Running}} agentplaza-postgres".to_string())
        );
        assert!(calls.contains(&"docker info --format {{.ServerVersion}}".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_denied_daemon_is_blocked_on_linux_only() {
        let daemon = Daemon::Denied;
        let check = docker_running(true, &daemon);
        if cfg!(target_os = "linux") {
            assert_eq!(check.state, State::Blocked);
        }
        assert_eq!(
            docker_running(false, &Daemon::NotAnswering).detail,
            "install Docker first"
        );
    }

    #[test]
    fn fixable_lists_only_the_missing_rows_with_a_fix() {
        let checks = vec![
            check(Need::Docker, "Docker", State::Ok, "27", None, true),
            check(
                Need::Postgres,
                "Postgres",
                State::Missing,
                "not found",
                Some("Create it".into()),
                true,
            ),
            check(
                Need::Services,
                "AgentPlaza services",
                State::Blocked,
                "missing",
                None,
                true,
            ),
            check(
                Need::Garage,
                "Garage",
                State::Missing,
                "stopped",
                Some("Start it".into()),
                false,
            ),
        ];
        let fixable: Vec<Need> = crate::fixable(&checks).iter().map(|c| c.need).collect();
        assert_eq!(fixable, vec![Need::Postgres, Need::Garage]);
    }
}
