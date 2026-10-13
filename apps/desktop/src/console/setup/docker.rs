//! Docker: installing it and starting its daemon, per platform (see `super`).
//!
//! Each command runs with stdin closed and its output captured: every line goes to the log as it
//! arrives, and a non-zero exit is an error naming the command and its last stderr line. On Linux,
//! root is asked for through `pkexec` (the desktop's own password prompt); nothing here reads a
//! password.

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use std::io::{BufRead, BufReader, Read};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use std::process::{Command, Stdio};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use std::sync::mpsc;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use std::thread;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use std::time::{Duration, Instant};
#[cfg(target_os = "macos")]
use std::path::Path;

/// Installs Docker for this platform, logging each step through `log`. Blocking.
#[cfg(target_os = "linux")]
pub fn install(log: &mut dyn FnMut(String)) -> Result<(), String> {
    if !on_path("pkexec") {
        return Err(
            "needs pkexec to ask for your password: install Docker with your package manager"
                .into(),
        );
    }
    let release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let manager = manager_for(&release).ok_or_else(|| {
        "no known package manager: install Docker from https://docs.docker.com/engine/install/"
            .to_string()
    })?;
    match manager {
        "apt" => {
            run(log, "pkexec", &["apt-get", "update"])?;
            run(
                log,
                "pkexec",
                &["env", "DEBIAN_FRONTEND=noninteractive", "apt-get", "install", "-y", "docker.io"],
            )?;
        }
        "dnf" => {
            run(log, "pkexec", &["dnf", "install", "-y", "moby-engine"])?;
        }
        "pacman" => {
            run(log, "pkexec", &["pacman", "-S", "--noconfirm", "docker"])?;
        }
        // zypper: the only other manager `manager_for` returns.
        _ => {
            run(log, "pkexec", &["zypper", "--non-interactive", "install", "docker"])?;
        }
    }
    run(log, "pkexec", &["systemctl", "enable", "--now", "docker"])?;
    let user = match std::env::var("USER") {
        Ok(user) if !user.is_empty() => user,
        _ => run(log, "id", &["-un"])?.trim().to_string(),
    };
    run(log, "pkexec", &["usermod", "-aG", "docker", user.as_str()])?;
    log("Docker is installed. Log out and back in once so your user can use it.".into());
    Ok(())
}

/// Installs Docker Desktop with Homebrew when present, else from Docker's DMG, then opens it.
#[cfg(target_os = "macos")]
pub fn install(log: &mut dyn FnMut(String)) -> Result<(), String> {
    match brew() {
        Some(program) => {
            run(log, program, &["install", "--cask", "docker"])?;
        }
        None => install_dmg(log)?,
    }
    run(log, "open", &["-a", "Docker"])?;
    log("Docker Desktop is installed and opening: finish its setup window, then come back here.".into());
    Ok(())
}

/// Installs Docker Desktop with winget (Windows asks for its own permission), then opens it.
#[cfg(target_os = "windows")]
pub fn install(log: &mut dyn FnMut(String)) -> Result<(), String> {
    if !on_path("winget") {
        return Err(
            "needs winget (App Installer from the Microsoft Store), or install Docker Desktop from https://www.docker.com/products/docker-desktop/"
                .into(),
        );
    }
    run(
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
    start_desktop(log)?;
    log("Docker Desktop is installed and opening: finish its setup window (it may ask to restart Windows), then come back here.".into());
    Ok(())
}

/// Installs Docker: not known on this platform.
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub fn install(_log: &mut dyn FnMut(String)) -> Result<(), String> {
    Err("install Docker by hand".into())
}

/// Starts Docker's daemon (the desktop app on macOS and Windows) and waits until it answers.
#[cfg(target_os = "linux")]
pub fn start(log: &mut dyn FnMut(String)) -> Result<(), String> {
    run(log, "pkexec", &["systemctl", "start", "docker"])?;
    wait_for_daemon(log)
}

/// Starts Docker Desktop and waits until its daemon answers.
#[cfg(target_os = "macos")]
pub fn start(log: &mut dyn FnMut(String)) -> Result<(), String> {
    run(log, "open", &["-a", "Docker"])?;
    wait_for_daemon(log)
}

/// Starts Docker Desktop and waits until its daemon answers.
#[cfg(target_os = "windows")]
pub fn start(log: &mut dyn FnMut(String)) -> Result<(), String> {
    start_desktop(log)?;
    wait_for_daemon(log)
}

/// Starts Docker's daemon: not known on this platform.
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub fn start(_log: &mut dyn FnMut(String)) -> Result<(), String> {
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
    } else if ids.iter().any(|id| id == "suse" || id.starts_with("opensuse")) {
        Some("zypper")
    } else {
        None
    }
}

/// Runs `program args` with stdin closed, logging `$ program args` first and then every output
/// line as it arrives (stdout and stderr). Returns the captured stdout on success; on a non-zero
/// exit, the error names the command and the last line it printed to stderr.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn run(log: &mut dyn FnMut(String), program: &str, args: &[&str]) -> Result<String, String> {
    let line = std::iter::once(program)
        .chain(args.iter().copied())
        .collect::<Vec<_>>()
        .join(" ");
    log(format!("$ {line}"));
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{line} failed: {e}"))?;
    // Both pipes are drained on their own threads (so neither can fill and block the child); the
    // lines reach `log` in the order they arrive.
    let (tx, rx) = mpsc::channel();
    let out = pump(child.stdout.take(), tx.clone());
    let err = pump(child.stderr.take(), tx);
    for text in rx {
        log(text);
    }
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    let status = child.wait().map_err(|e| format!("{line} failed: {e}"))?;
    if status.success() {
        Ok(stdout.join("\n"))
    } else {
        let last = stderr
            .iter()
            .rev()
            .find(|l| !l.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| status.to_string());
        Err(format!("{line} failed: {last}"))
    }
}

/// Reads `reader` line by line, sending each line to `tx` as it comes; the thread returns all
/// the lines when the stream ends.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn pump<R: Read + Send + 'static>(
    reader: Option<R>,
    tx: mpsc::Sender<String>,
) -> thread::JoinHandle<Vec<String>> {
    thread::spawn(move || {
        let mut lines = Vec::new();
        if let Some(reader) = reader {
            for line in BufReader::new(reader).lines().map_while(Result::ok) {
                let _ = tx.send(line.clone());
                lines.push(line);
            }
        }
        lines
    })
}

/// Whether `program` is on PATH (nothing is run).
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn on_path(program: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        dir.join(program).is_file() || dir.join(format!("{program}.exe")).is_file()
    })
}

/// Polls `docker info` every 2 seconds for up to 2 minutes, logging "waiting for Docker…" every
/// 10 seconds. Ends early when this user has no access to the daemon (it was just added to the
/// docker group, which only a new login picks up).
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn wait_for_daemon(log: &mut dyn FnMut(String)) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut next_note = Instant::now() + Duration::from_secs(10);
    loop {
        match docker_version() {
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
fn docker_version() -> Result<String, String> {
    let out = Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !version.is_empty() {
            return Ok(version);
        }
    }
    Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
}

/// Opens Docker Desktop with `cmd /C start` (Windows), as the desktop app would.
#[cfg(target_os = "windows")]
fn start_desktop(log: &mut dyn FnMut(String)) -> Result<(), String> {
    run(
        log,
        "cmd",
        &["/C", "start", "", r"C:\Program Files\Docker\Docker\Docker Desktop.exe"],
    )?;
    Ok(())
}

/// Homebrew's `brew` program: on PATH, or in its usual install folders.
#[cfg(target_os = "macos")]
fn brew() -> Option<&'static str> {
    if on_path("brew") {
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
fn install_dmg(log: &mut dyn FnMut(String)) -> Result<(), String> {
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
    run(log, "curl", &["-fL", "-o", dmg.as_str(), url])?;
    run(
        log,
        "hdiutil",
        &["attach", "-nobrowse", "-readonly", "-mountpoint", mnt.as_str(), dmg.as_str()],
    )?;
    let app = format!("{mnt}/Docker.app");
    let copied = run(log, "ditto", &[app.as_str(), "/Applications/Docker.app"]);
    let detached = run(log, "hdiutil", &["detach", mnt.as_str()]);
    let _ = std::fs::remove_dir_all(&dir);
    copied?;
    detached?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::manager_for;

    const UBUNTU: &str = "PRETTY_NAME=\"Ubuntu 24.04 LTS\"\nNAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nID=ubuntu\nID_LIKE=debian\n";
    const RASPBIAN: &str = "PRETTY_NAME=\"Raspbian GNU/Linux 12 (bookworm)\"\nNAME=\"Raspbian GNU/Linux\"\nID=raspbian\nID_LIKE=debian\n";
    const FEDORA: &str = "NAME=\"Fedora Linux\"\nVERSION_ID=41\nID=fedora\nPLATFORM_ID=\"platform:f41\"\n";
    const RHEL: &str = "NAME=\"Red Hat Enterprise Linux\"\nID=\"rhel\"\nID_LIKE=\"fedora\"\n";
    const ARCH: &str = "NAME=\"Arch Linux\"\nID=arch\nBUILD_ID=rolling\n";
    const TUMBLEWEED: &str = "NAME=\"openSUSE Tumbleweed\"\nID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n";
    const GENTOO: &str = "NAME=Gentoo\nID=gentoo\n";

    #[test]
    fn ubuntu_uses_apt() {
        assert_eq!(manager_for(UBUNTU), Some("apt"));
    }

    #[test]
    fn raspbian_uses_apt() {
        assert_eq!(manager_for(RASPBIAN), Some("apt"));
    }

    #[test]
    fn fedora_uses_dnf() {
        assert_eq!(manager_for(FEDORA), Some("dnf"));
    }

    #[test]
    fn rhel_uses_dnf_through_id_like() {
        assert_eq!(manager_for(RHEL), Some("dnf"));
    }

    #[test]
    fn arch_uses_pacman() {
        assert_eq!(manager_for(ARCH), Some("pacman"));
    }

    #[test]
    fn opensuse_uses_zypper() {
        assert_eq!(manager_for(TUMBLEWEED), Some("zypper"));
    }

    #[test]
    fn unknown_distro_has_no_manager() {
        assert_eq!(manager_for(GENTOO), None);
        assert_eq!(manager_for(""), None);
    }
}
