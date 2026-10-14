//! Installing Docker and starting its daemon, per platform (see `lib.rs`). Every command goes
//! through the [`Runner`](crate::Runner) of the [`Env`](crate::Env).

use crate::Env;
#[cfg(target_os = "macos")]
use std::path::Path;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use std::thread;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use std::time::{Duration, Instant};

/// Installs Docker for this platform, logging each step through `log`. Blocking.
#[cfg(target_os = "linux")]
pub fn install(env: &Env, log: &mut dyn FnMut(String)) -> Result<(), String> {
    if !env.runner.on_path("pkexec") {
        return Err(
            "needs pkexec to ask for your password: install Docker with your package manager"
                .into(),
        );
    }
    let release = env.runner.read_file("/etc/os-release").unwrap_or_default();
    let manager = manager_for(&release).ok_or_else(|| {
        "no known package manager: install Docker from https://docs.docker.com/engine/install/"
            .to_string()
    })?;
    match manager {
        "apt" => {
            run(env, log, "pkexec", &["apt-get", "update"])?;
            run(
                env,
                log,
                "pkexec",
                &[
                    "env",
                    "DEBIAN_FRONTEND=noninteractive",
                    "apt-get",
                    "install",
                    "-y",
                    "docker.io",
                ],
            )?;
        }
        "dnf" => {
            run(env, log, "pkexec", &["dnf", "install", "-y", "moby-engine"])?;
        }
        "pacman" => {
            run(
                env,
                log,
                "pkexec",
                &["pacman", "-S", "--noconfirm", "docker"],
            )?;
        }
        // zypper: the only other manager `manager_for` returns.
        _ => {
            run(
                env,
                log,
                "pkexec",
                &["zypper", "--non-interactive", "install", "docker"],
            )?;
        }
    }
    run(
        env,
        log,
        "pkexec",
        &["systemctl", "enable", "--now", "docker"],
    )?;
    let user = match &env.user {
        Some(user) => user.clone(),
        None => run(env, log, "id", &["-un"])?.trim().to_string(),
    };
    run(
        env,
        log,
        "pkexec",
        &["usermod", "-aG", "docker", user.as_str()],
    )?;
    log("Docker is installed. Log out and back in once so your user can use it.".into());
    Ok(())
}

/// Installs Docker Desktop with Homebrew when present, else from Docker's DMG, then opens it.
#[cfg(target_os = "macos")]
pub fn install(env: &Env, log: &mut dyn FnMut(String)) -> Result<(), String> {
    match brew(env) {
        Some(program) => {
            run(env, log, program, &["install", "--cask", "docker"])?;
        }
        None => install_dmg(env, log)?,
    }
    run(env, log, "open", &["-a", "Docker"])?;
    log(
        "Docker Desktop is installed and opening: finish its setup window, then come back here."
            .into(),
    );
    Ok(())
}

/// Installs Docker Desktop with winget (Windows asks for its own permission), then opens it.
#[cfg(target_os = "windows")]
pub fn install(env: &Env, log: &mut dyn FnMut(String)) -> Result<(), String> {
    if !env.runner.on_path("winget") {
        return Err(
            "needs winget (App Installer from the Microsoft Store), or install Docker Desktop from https://www.docker.com/products/docker-desktop/"
                .into(),
        );
    }
    run(
        env,
        log,
        "winget",
        &[
            "install",
            "-e",
            "--id",
            "Docker.DockerDesktop",
            "--accept-package-agreements",
            "--accept-source-agreements",
        ],
    )?;
    start_desktop(env, log)?;
    log("Docker Desktop is installed and opening: finish its setup window (it may ask to restart Windows), then come back here.".into());
    Ok(())
}

/// Installs Docker: not known on this platform.
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub fn install(_env: &Env, _log: &mut dyn FnMut(String)) -> Result<(), String> {
    Err("install Docker by hand".into())
}

/// Starts Docker's daemon (the desktop app on macOS and Windows) and waits until it answers.
#[cfg(target_os = "linux")]
pub fn start(env: &Env, log: &mut dyn FnMut(String)) -> Result<(), String> {
    run(env, log, "pkexec", &["systemctl", "start", "docker"])?;
    wait_for_daemon(env, log)
}

/// Starts Docker Desktop and waits until its daemon answers.
#[cfg(target_os = "macos")]
pub fn start(env: &Env, log: &mut dyn FnMut(String)) -> Result<(), String> {
    run(env, log, "open", &["-a", "Docker"])?;
    wait_for_daemon(env, log)
}

/// Starts Docker Desktop and waits until its daemon answers.
#[cfg(target_os = "windows")]
pub fn start(env: &Env, log: &mut dyn FnMut(String)) -> Result<(), String> {
    start_desktop(env, log)?;
    wait_for_daemon(env, log)
}

/// Starts Docker's daemon: not known on this platform.
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub fn start(_env: &Env, _log: &mut dyn FnMut(String)) -> Result<(), String> {
    Err("start Docker by hand".into())
}

/// The package manager for an `/etc/os-release` text, by its `ID` and `ID_LIKE`: `apt`, `dnf`,
/// `pacman` or `zypper`, or `None` when none of them is known.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn manager_for(os_release: &str) -> Option<&'static str> {
    let mut ids: Vec<String> = Vec::new();
    for line in os_release.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key == "ID" || key == "ID_LIKE" {
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
            ids.extend(value.split_whitespace().map(str::to_ascii_lowercase));
        }
    }
    let has = |names: &[&str]| ids.iter().any(|id| names.contains(&id.as_str()));
    if has(&["debian", "ubuntu", "raspbian"]) {
        Some("apt")
    } else if has(&["fedora", "rhel", "centos"]) {
        Some("dnf")
    } else if has(&["arch"]) {
        Some("pacman")
    } else if ids
        .iter()
        .any(|id| id == "suse" || id.starts_with("opensuse"))
    {
        Some("zypper")
    } else {
        None
    }
}

/// Runs `program args` through the runner, logging `$ program args` first and then every output
/// line as it arrives. Returns the captured stdout on success; on a non-zero exit, the error names
/// the command and the last line it printed to stderr.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn run(
    env: &Env,
    log: &mut dyn FnMut(String),
    program: &str,
    args: &[&str],
) -> Result<String, String> {
    let line = std::iter::once(program)
        .chain(args.iter().copied())
        .collect::<Vec<_>>()
        .join(" ");
    log(format!("$ {line}"));
    let out = env
        .runner
        .stream(program, args, log)
        .map_err(|e| format!("{line} failed: {e}"))?;
    if out.success {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let last = stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| match out.code {
                Some(code) => format!("exit status {code}"),
                None => "exit status unknown".to_string(),
            });
        Err(format!("{line} failed: {last}"))
    }
}

/// Polls `docker info` every 2 seconds for up to 2 minutes, logging "waiting for Docker…" every
/// 10 seconds. Ends early when this user has no access to the daemon (it was just added to the
/// docker group, which only a new login picks up).
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn wait_for_daemon(env: &Env, log: &mut dyn FnMut(String)) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut next_note = Instant::now() + Duration::from_secs(10);
    loop {
        match docker_version(env) {
            Ok(version) => {
                log(format!("Docker {version} is running"));
                return Ok(());
            }
            Err(e) if e.to_ascii_lowercase().contains("permission denied") => {
                return Err(
                    "Docker is up, but this user cannot use it yet: log out and back in once, then try again"
                        .into(),
                );
            }
            Err(_) => {}
        }
        let now = Instant::now();
        if now >= deadline {
            return Err("Docker did not answer within 2 minutes: open Docker and try again".into());
        }
        if now >= next_note {
            log("waiting for Docker…".into());
            next_note += Duration::from_secs(10);
        }
        thread::sleep(Duration::from_secs(2));
    }
}

/// The daemon's version from `docker info`, or what `docker info` printed to stderr when it does
/// not answer. Nothing is logged for these probes.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn docker_version(env: &Env) -> Result<String, String> {
    let out = env
        .runner
        .output("docker", &["info", "--format", "{{.ServerVersion}}"])
        .map_err(|e| e.to_string())?;
    if out.success {
        let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !version.is_empty() {
            return Ok(version);
        }
    }
    Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
}

/// Opens Docker Desktop with `cmd /C start` (Windows), as the desktop app would.
#[cfg(target_os = "windows")]
fn start_desktop(env: &Env, log: &mut dyn FnMut(String)) -> Result<(), String> {
    run(
        env,
        log,
        "cmd",
        &[
            "/C",
            "start",
            "",
            r"C:\Program Files\Docker\Docker\Docker Desktop.exe",
        ],
    )?;
    Ok(())
}

/// Homebrew's `brew` program: on PATH, or in its usual install folders.
#[cfg(target_os = "macos")]
fn brew(env: &Env) -> Option<&'static str> {
    if env.runner.on_path("brew") {
        return Some("brew");
    }
    ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]
        .iter()
        .copied()
        .find(|p| Path::new(p).exists())
}

/// Downloads Docker Desktop's DMG for this CPU, mounts it, copies Docker.app to /Applications
/// and unmounts it again.
#[cfg(target_os = "macos")]
fn install_dmg(env: &Env, log: &mut dyn FnMut(String)) -> Result<(), String> {
    let url = if cfg!(target_arch = "aarch64") {
        "https://desktop.docker.com/mac/main/arm64/Docker.dmg"
    } else {
        "https://desktop.docker.com/mac/main/amd64/Docker.dmg"
    };
    let dir = std::env::temp_dir().join(format!("agentplaza-docker-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("mnt"))
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let dmg = dir.join("Docker.dmg").display().to_string();
    let mnt = dir.join("mnt").display().to_string();
    run(env, log, "curl", &["-fL", "-o", dmg.as_str(), url])?;
    run(
        env,
        log,
        "hdiutil",
        &[
            "attach",
            "-nobrowse",
            "-readonly",
            "-mountpoint",
            mnt.as_str(),
            dmg.as_str(),
        ],
    )?;
    let app = format!("{mnt}/Docker.app");
    let copied = run(
        env,
        log,
        "ditto",
        &[app.as_str(), "/Applications/Docker.app"],
    );
    let detached = run(env, log, "hdiutil", &["detach", mnt.as_str()]);
    let _ = std::fs::remove_dir_all(&dir);
    copied?;
    detached?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::manager_for;
    #[cfg(target_os = "linux")]
    use super::{install, start};
    #[cfg(target_os = "linux")]
    use crate::Env;
    #[cfg(target_os = "linux")]
    use crate::runner::Output;
    #[cfg(target_os = "linux")]
    use crate::runner::mock::Mock;

    const UBUNTU: &str = "PRETTY_NAME=\"Ubuntu 24.04 LTS\"\nNAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nID=ubuntu\nID_LIKE=debian\n";
    const RASPBIAN: &str = "PRETTY_NAME=\"Raspbian GNU/Linux 12 (bookworm)\"\nNAME=\"Raspbian GNU/Linux\"\nID=raspbian\nID_LIKE=debian\n";
    const FEDORA: &str =
        "NAME=\"Fedora Linux\"\nVERSION_ID=41\nID=fedora\nPLATFORM_ID=\"platform:f41\"\n";
    const RHEL: &str = "NAME=\"Red Hat Enterprise Linux\"\nID=\"rhel\"\nID_LIKE=\"fedora\"\n";
    const ARCH: &str = "NAME=\"Arch Linux\"\nID=arch\nBUILD_ID=rolling\n";
    const TUMBLEWEED: &str =
        "NAME=\"openSUSE Tumbleweed\"\nID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n";
    const GENTOO: &str = "NAME=Gentoo\nID=gentoo\n";

    #[test]
    fn manager_for_maps_each_distro_to_its_package_manager() {
        for (case, os_release, want) in [
            ("ubuntu", UBUNTU, Some("apt")),
            ("raspbian", RASPBIAN, Some("apt")),
            ("fedora", FEDORA, Some("dnf")),
            ("rhel, through ID_LIKE", RHEL, Some("dnf")),
            ("arch", ARCH, Some("pacman")),
            ("opensuse-tumbleweed", TUMBLEWEED, Some("zypper")),
            ("gentoo, no known manager", GENTOO, None),
            ("empty os-release", "", None),
        ] {
            assert_eq!(manager_for(os_release), want, "{case}");
        }
    }

    /// Every command succeeds; `docker info` answers with a version.
    #[cfg(target_os = "linux")]
    fn all_succeed(program: &str, _args: &[&str]) -> Output {
        match program {
            "docker" => Output::ok("27.1.1\n"),
            _ => Output::ok(""),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn apt_install_runs_the_steps_in_order_as_root_and_adds_the_user_to_docker() {
        let mut mock = Mock::new(all_succeed);
        mock.paths = vec!["pkexec"];
        mock.files = vec![("/etc/os-release", UBUNTU.to_string())];
        let env = Env {
            runner: &mock,
            keys: None,
            user: Some("tester".into()),
        };
        let mut lines = Vec::new();
        install(&env, &mut |l| lines.push(l)).unwrap();
        assert_eq!(
            mock.calls(),
            vec![
                "pkexec apt-get update",
                "pkexec env DEBIAN_FRONTEND=noninteractive apt-get install -y docker.io",
                "pkexec systemctl enable --now docker",
                "pkexec usermod -aG docker tester",
            ]
        );
        assert!(lines.last().unwrap().starts_with("Docker is installed"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn install_asks_for_pkexec_first_and_runs_nothing_without_it() {
        let mock = Mock::new(all_succeed);
        let env = Env {
            runner: &mock,
            keys: None,
            user: Some("tester".into()),
        };
        assert!(install(&env, &mut |_| {}).is_err());
        assert!(mock.calls().is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_failed_step_stops_the_install_with_its_last_error_line() {
        let mut mock = Mock::new(|program, args| {
            if args.contains(&"DEBIAN_FRONTEND=noninteractive") {
                Output::failed("E: Unable to locate package docker.io\n")
            } else {
                all_succeed(program, args)
            }
        });
        mock.paths = vec!["pkexec"];
        mock.files = vec![("/etc/os-release", UBUNTU.to_string())];
        let env = Env {
            runner: &mock,
            keys: None,
            user: Some("tester".into()),
        };
        let err = install(&env, &mut |_| {}).unwrap_err();
        assert!(
            err.contains("E: Unable to locate package docker.io"),
            "{err}"
        );
        assert_eq!(
            mock.calls().len(),
            2,
            "the install stops at the failed step"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn start_restarts_the_daemon_and_waits_until_it_answers() {
        let mock = Mock::new(all_succeed);
        let env = Env {
            runner: &mock,
            keys: None,
            user: None,
        };
        let mut lines = Vec::new();
        start(&env, &mut |l| lines.push(l)).unwrap();
        assert_eq!(
            mock.calls(),
            vec![
                "pkexec systemctl start docker",
                "docker info --format {{.ServerVersion}}"
            ]
        );
        assert!(lines.contains(&"Docker 27.1.1 is running".to_string()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn install_reads_the_login_name_when_the_environment_has_none() {
        let mut mock = Mock::new(|program, args| match (program, args.first()) {
            ("id", _) => Output::ok("carol\n"),
            _ => all_succeed(program, args),
        });
        mock.paths = vec!["pkexec"];
        mock.files = vec![("/etc/os-release", FEDORA.to_string())];
        let env = Env {
            runner: &mock,
            keys: None,
            user: None,
        };
        install(&env, &mut |_| {}).unwrap();
        let calls = mock.calls();
        assert!(calls.contains(&"id -un".to_string()));
        assert_eq!(calls.last().unwrap(), "pkexec usermod -aG docker carol");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn install_stops_on_an_unknown_distro_before_running_anything() {
        let mut mock = Mock::new(all_succeed);
        mock.paths = vec!["pkexec"];
        mock.files = vec![("/etc/os-release", GENTOO.to_string())];
        let env = Env {
            runner: &mock,
            keys: None,
            user: Some("tester".into()),
        };
        assert!(install(&env, &mut |_| {}).is_err());
        assert!(mock.calls().is_empty());
    }
}
