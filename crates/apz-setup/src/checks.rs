//! What a plaza needs on this machine, checked for its [`Mode`](crate::Mode) (see `lib.rs`).

use crate::node;
use crate::runner::Output;
use crate::{Check, Env, Mode, Need, State};
use std::net::TcpListener;
use std::path::{Path, PathBuf};

const WHP_QUERY: &str =
    "(Get-CimInstance Win32_OptionalFeature -Filter \"Name='HypervisorPlatform'\").InstallState";

/// The services `mode` runs, in start order, on a platform that runs all of them. A node's are apz-link (it
/// forwards to the hub), vsockd and noded: the node's plan ([`node::NAMES`]).
fn services_for(mode: Mode) -> &'static [&'static str] {
    match mode {
        Mode::Local => &["apid", "vsockd", "noded"],
        Mode::Hub => &["apid"],
        Mode::Node => &node::NAMES,
    }
}

/// Whether a machine of `os` and `arch` (`std::env::consts`) runs service `name`. The VM node (noded, vsockd) is
/// staged for Linux, Apple silicon and Windows x64 (`packaging/scripts/stage.sh`), and vsockd's guest backend does not
/// run on macOS yet (it comes with the Hypervisor.framework node, PLAN 7.1); apid and apz-link run everywhere.
pub fn runs_on(name: &str, os: &str, arch: &str) -> bool {
    let vm_node = os == "linux"
        || (os == "macos" && arch == "aarch64")
        || (os == "windows" && arch == "x86_64");
    match name {
        "noded" => vm_node,
        "vsockd" => vm_node && os != "macos",
        _ => true,
    }
}

/// [`runs_on`] for this machine.
pub fn runs_here(name: &str) -> bool {
    runs_on(name, std::env::consts::OS, std::env::consts::ARCH)
}

/// Why this machine does not run service `name`, for a status line.
pub fn not_run_here(name: &str) -> &'static str {
    if name == "vsockd" && cfg!(target_os = "macos") {
        "not run on macOS yet: vsockd's guest backend comes with the Mac's VM node"
    } else {
        "not built for this platform: the VM node ships on Linux, Apple silicon and Windows x64"
    }
}

/// The port service `name` listens on: apid's 7700, apz-link's forwarder on 7701 (vsockd reaches apid through it),
/// noded's 8899 and vsockd's HTTP 19080.
fn port_of(name: &str) -> u16 {
    match name {
        "apz-link" => 7701,
        "noded" => 8899,
        "vsockd" => 19080,
        _ => 7700,
    }
}

/// The services `mode` runs on a machine of `os` and `arch`.
fn services_on(mode: Mode, os: &str, arch: &str) -> Vec<&'static str> {
    services_for(mode)
        .iter()
        .copied()
        .filter(|name| runs_on(name, os, arch))
        .collect()
}

/// The ports `mode`'s services listen on, on a machine of `os` and `arch`, ascending.
fn ports_on(mode: Mode, os: &str, arch: &str) -> Vec<u16> {
    let mut ports: Vec<u16> = services_on(mode, os, arch)
        .into_iter()
        .map(port_of)
        .collect();
    ports.sort_unstable();
    ports
}

/// Every requirement for `mode` on this machine, in the order the checklist shows them.
pub fn check_all(mode: Mode) -> Vec<Check> {
    let env = Env::system();
    check_with(
        mode,
        &search_dirs(),
        || hypervisor(&env),
        |wanted| ports(mode, wanted),
    )
}

/// The rows for `mode`, with the plaza's binaries searched for in `dirs`. The hypervisor row comes from
/// `probe_hypervisor`, and only for a mode that runs a node: a hub-only check never asks. The ports row
/// comes from `probe_ports`, given the ports of the mode (see `ports_for`). Both are passed in, so a
/// test does not probe the machine it runs on.
pub fn check_with(
    mode: Mode,
    dirs: &[PathBuf],
    probe_hypervisor: impl FnOnce() -> Check,
    probe_ports: impl FnOnce(&[u16]) -> Check,
) -> Vec<Check> {
    let mut rows = vec![services(mode, dirs)];
    if mode.runs_node() {
        rows.push(probe_hypervisor());
    }
    rows.push(probe_ports(&ports_on(
        mode,
        std::env::consts::OS,
        std::env::consts::ARCH,
    )));
    rows
}

/// Builds a row. The setup installs nothing, so no row carries a fix.
fn check(
    need: Need,
    label: &'static str,
    state: State,
    detail: impl Into<String>,
    required: bool,
) -> Check {
    Check {
        need,
        label,
        state,
        detail: detail.into(),
        fix: None,
        required,
    }
}

/// The output of `program args` through the runner; `None` if it cannot run at all.
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

/// The services of `mode`, looked for in `dirs`: all of them there, or the missing ones named.
fn services(mode: Mode, dirs: &[PathBuf]) -> Check {
    // Only what this platform runs (`runs_on`): a Mac runs no vsockd yet, an Intel Mac and Windows on Arm no VM node.
    let needed = services_on(mode, std::env::consts::OS, std::env::consts::ARCH);
    let found: Vec<(&str, Option<PathBuf>)> = needed
        .iter()
        .map(|name| (*name, locate(name, dirs)))
        .collect();
    let missing: Vec<&str> = found
        .iter()
        .filter(|(_, path)| path.is_none())
        .map(|(name, _)| *name)
        .collect();
    if !missing.is_empty() {
        return check(
            Need::Services,
            "AgentPlaza services",
            State::Blocked,
            format!("missing: {}; reinstall AgentPlaza", missing.join(", ")),
            true,
        );
    }
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
        format!(
            "{} in {dir}; no Docker or database needed",
            needed.join(", ")
        ),
        true,
    )
}

/// This machine's hypervisor for the agents' VMs. Optional: never required, so a missing one is only
/// reported.
fn hypervisor(env: &Env) -> Check {
    let (state, detail) = hypervisor_state(env);
    check(Need::Hypervisor, "Hypervisor", state, detail, false)
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

/// The ports row for `wanted`, the ports of `mode`: free; held by AgentPlaza itself (see
/// `running_here`); or held by something else, which blocks the start.
fn ports(mode: Mode, wanted: &[u16]) -> Check {
    let (state, detail) = match busy_port(wanted) {
        None => (State::Ok, format!("{} free", port_list(wanted))),
        Some(_) if running_here(mode) => {
            (State::Ok, "AgentPlaza is already running here".to_string())
        }
        Some(port) => (
            State::Blocked,
            format!("port {port} is in use: stop what holds it"),
        ),
    };
    check(Need::Ports, "Ports", state, detail, true)
}

/// Whether AgentPlaza's own services answer here, which makes a busy port its own: the hub's overview
/// for a hub, the node's processes for a node, either for a local plaza.
fn running_here(mode: Mode) -> bool {
    match mode {
        Mode::Local => plaza_running() || node_running(),
        Mode::Hub => plaza_running(),
        Mode::Node => node_running(),
    }
}

/// The ports as a comma list: `7700, 8899, 19080`.
fn port_list(ports: &[u16]) -> String {
    ports
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The first port of `ports` another process holds on 127.0.0.1.
fn busy_port(ports: &[u16]) -> Option<u16> {
    ports
        .iter()
        .copied()
        .find(|port| TcpListener::bind(("127.0.0.1", *port)).is_err())
}

/// Whether AgentPlaza's own hub answers on 127.0.0.1:7700 (its overview names the agents): then the
/// plaza is already running and its ports are held by it, not by something in the way.
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

/// Whether this machine's node runs: one of the processes its plan names (`node::find_plan_pids`). A
/// machine that has not joined a hub has no node.
fn node_running() -> bool {
    let Some((base, count)) = node::identity() else {
        return false;
    };
    node::plan(base, count, &node::Layout::current())
        .iter()
        .any(|process| !node::find_plan_pids(process).is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// An empty directory for a test's files: `apz-setup-<tag>` under the system's temp dir (each test
    /// has its own tag).
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-setup-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A directory holding the binaries `names`, as the plaza ships them.
    fn shipped(tag: &str, names: &[&str]) -> PathBuf {
        let dir = scratch(tag);
        for name in names {
            fs::write(dir.join(apz_os::process::exe(name)), b"").unwrap();
        }
        dir
    }

    /// A ports row that does not probe the machine.
    fn fixed_ports(_wanted: &[u16]) -> Check {
        check(Need::Ports, "Ports", State::Ok, "fixed for the test", true)
    }

    /// A hypervisor row that does not probe the machine.
    fn fixed_hypervisor() -> Check {
        check(
            Need::Hypervisor,
            "Hypervisor",
            State::Warning,
            "fixed for the test",
            false,
        )
    }

    fn needs(checks: &[Check]) -> Vec<Need> {
        checks.iter().map(|check| check.need).collect()
    }

    fn row(checks: &[Check], need: Need) -> &Check {
        checks.iter().find(|check| check.need == need).unwrap()
    }

    #[test]
    fn a_bound_port_counts_as_busy() {
        // (Not "free once released": tests run in parallel, and another may take the port first.)
        let held = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        assert_eq!(busy_port(&[port]), Some(port));
    }

    #[test]
    fn each_mode_asks_for_its_own_services_and_ports() {
        let linux = |mode| {
            (
                services_on(mode, "linux", "x86_64"),
                ports_on(mode, "linux", "x86_64"),
            )
        };
        assert_eq!(linux(Mode::Hub), (vec!["apid"], vec![7700u16]));
        assert_eq!(
            linux(Mode::Local),
            (vec!["apid", "vsockd", "noded"], vec![7700u16, 8899, 19080])
        );
        assert_eq!(
            linux(Mode::Node),
            (
                vec!["apz-link", "vsockd", "noded"],
                vec![7701u16, 8899, 19080]
            )
        );
    }

    #[test]
    fn a_platform_is_asked_only_for_the_services_it_runs() {
        // Apple silicon runs the hub and noded; vsockd's guest backend is not there yet.
        assert_eq!(
            services_on(Mode::Local, "macos", "aarch64"),
            ["apid", "noded"]
        );
        assert_eq!(ports_on(Mode::Local, "macos", "aarch64"), [7700u16, 8899]);
        // An Intel Mac and Windows on Arm run no VM node: the hub alone (and apz-link for a node).
        assert_eq!(services_on(Mode::Local, "macos", "x86_64"), ["apid"]);
        assert_eq!(services_on(Mode::Local, "windows", "aarch64"), ["apid"]);
        assert_eq!(services_on(Mode::Node, "windows", "aarch64"), ["apz-link"]);
        assert_eq!(
            services_on(Mode::Local, "windows", "x86_64"),
            ["apid", "vsockd", "noded"]
        );
    }

    #[test]
    fn a_hub_only_check_has_no_node_daemon_no_hypervisor_and_no_node_port() {
        let dir = shipped("checks-hub", &["apid", "vsockd", "noded"]);
        let checks = check_with(
            Mode::Hub,
            std::slice::from_ref(&dir),
            || panic!("a hub-only check must not probe the hypervisor"),
            |wanted| {
                assert_eq!(wanted, &[7700u16], "a hub asks for its own port only");
                fixed_ports(wanted)
            },
        );
        assert_eq!(needs(&checks), [Need::Services, Need::Ports]);
        let services = row(&checks, Need::Services);
        assert_eq!(services.state, State::Ok, "{}", services.detail);
        assert!(services.detail.starts_with("apid"), "{}", services.detail);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_local_check_has_the_services_the_hypervisor_and_both_ports() {
        let dir = shipped("checks-local", &["apid", "vsockd", "noded"]);
        let checks = check_with(
            Mode::Local,
            std::slice::from_ref(&dir),
            fixed_hypervisor,
            |wanted| {
                assert_eq!(
                    wanted,
                    ports_on(Mode::Local, std::env::consts::OS, std::env::consts::ARCH)
                );
                fixed_ports(wanted)
            },
        );
        assert_eq!(
            needs(&checks),
            [Need::Services, Need::Hypervisor, Need::Ports]
        );
        assert_eq!(row(&checks, Need::Services).state, State::Ok);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_node_check_has_no_hub_port_and_no_apid() {
        let dir = shipped("checks-node", &["apz-link", "vsockd", "noded"]);
        let checks = check_with(
            Mode::Node,
            std::slice::from_ref(&dir),
            fixed_hypervisor,
            |wanted| {
                assert!(
                    !wanted.contains(&7700),
                    "a node-only check does not ask for the hub's port: {wanted:?}"
                );
                fixed_ports(wanted)
            },
        );
        assert_eq!(
            needs(&checks),
            [Need::Services, Need::Hypervisor, Need::Ports]
        );
        let services = row(&checks, Need::Services);
        assert_eq!(services.state, State::Ok, "{}", services.detail);
        assert!(!services.detail.contains("apid"), "{}", services.detail);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn each_mode_looks_for_its_own_services_only() {
        let hub = shipped("checks-services-hub", &["apid"]);
        assert_eq!(
            services(Mode::Hub, std::slice::from_ref(&hub)).state,
            State::Ok
        );
        // A node also needs apz-link: apid alone does not make one.
        let node = services(Mode::Node, std::slice::from_ref(&hub));
        assert_eq!(node.state, State::Blocked, "{}", node.detail);
        assert!(node.detail.contains("apz-link"), "{}", node.detail);
        let _ = fs::remove_dir_all(&hub);

        let plaza = shipped("checks-services-plaza", &["apid", "vsockd", "noded"]);
        assert_eq!(
            services(Mode::Local, std::slice::from_ref(&plaza)).state,
            State::Ok
        );
        let node = services(Mode::Node, std::slice::from_ref(&plaza));
        assert_eq!(node.state, State::Blocked, "{}", node.detail);
        assert!(node.detail.contains("apz-link"), "{}", node.detail);
        let _ = fs::remove_dir_all(&plaza);
    }

    #[test]
    fn a_missing_service_blocks_with_no_fix_and_names_it() {
        let check = services(Mode::Local, &[]);
        assert_eq!(check.state, State::Blocked);
        assert!(check.detail.starts_with("missing: "), "{}", check.detail);
        assert!(check.detail.contains("apid"), "{}", check.detail);
        assert!(check.fix.is_none());
    }

    #[test]
    fn the_hypervisor_row_is_never_required() {
        let env = Env::system();
        let checks = check_with(Mode::Local, &[], || hypervisor(&env), fixed_ports);
        assert!(!row(&checks, Need::Hypervisor).required);
    }

    #[test]
    fn check_all_for_a_hub_asks_for_no_node_and_no_hypervisor() {
        let checks = check_all(Mode::Hub);
        assert_eq!(needs(&checks), [Need::Services, Need::Ports]);
        let ports = row(&checks, Need::Ports);
        assert!(
            !ports.detail.contains("8899") && !ports.detail.contains("19080"),
            "{}",
            ports.detail
        );
    }

    #[test]
    fn check_all_for_a_node_asks_for_no_hub_port() {
        let checks = check_all(Mode::Node);
        assert_eq!(
            needs(&checks),
            [Need::Services, Need::Hypervisor, Need::Ports]
        );
        let ports = row(&checks, Need::Ports);
        assert!(!ports.detail.contains("7700"), "{}", ports.detail);
    }

    #[test]
    fn the_setup_names_no_docker_database_or_object_store_setting() {
        // Every row of every mode, and every answer of fix, with the plaza's binaries present. (The
        // scratch directory's name must not contain a banned word: its path can show in a row.)
        let dir = shipped("checks-scan", &["apid", "vsockd", "noded", "apz-link"]);
        let env = Env::system();
        let mut said = Vec::new();
        for mode in [Mode::Local, Mode::Hub, Mode::Node] {
            let checks = check_with(
                mode,
                std::slice::from_ref(&dir),
                || hypervisor(&env),
                |wanted| ports(mode, wanted),
            );
            for check in checks {
                said.push(check.detail);
                said.extend(check.fix);
            }
        }
        for need in Need::ALL {
            let mut logged = Vec::new();
            if let Err(err) = crate::fix(need, &mut |line| logged.push(line)) {
                said.push(err);
            }
            said.extend(logged);
        }
        for text in &said {
            // The one place Docker may be named is the statement that it is not needed.
            let lower = text
                .to_ascii_lowercase()
                .replace("no docker or database needed", "");
            for banned in [
                "docker",
                "postgres",
                "garage",
                "apz_database_url",
                "apz_s3_",
            ] {
                assert!(!lower.contains(banned), "{text:?} names {banned}");
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
