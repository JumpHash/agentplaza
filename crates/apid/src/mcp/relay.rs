//! The relay container (docs/design/mcp.md, "Running a server").
//!
//! MCP containers attach only to `apz-mcp`, which is internal: no route out, no host address. Their proxy address is
//! the relay `apz-mcp-relay`, attached to `apz-mcp` at a fixed address and to a normal bridge, `apz-mcp-uplink`,
//! from which it reaches the host as `host.docker.internal` (`--add-host=host.docker.internal:host-gateway`: Docker
//! Desktop provides the name, and a native Linux engine maps it to the bridge gateway).
//!
//! The relay runs `mcp_relay`, our static binary built for musl. apid copies it into its state directory
//! (`<state>/mcp/mcp_relay`) at each start, so Docker can mount it from a shared path, and mounts it read-only into
//! `alpine:latest`. The relay has no policy: it sends apid the session token and a PROXY line, then copies bytes.
//!
//! Lifecycle: apid ensures the networks, installs the binary and starts the relay before the first stdio server
//! starts. The relay is started with `docker start -a -i`, so its stdin is a pipe apid holds: when apid exits, the
//! pipe closes and the relay exits. Dropping the handle removes the container; each start removes a stale one first.

use std::io::{BufRead, BufReader};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use super::stdio::{Docker, NETWORK, NO_DAEMON, NO_DOCKER};

/// The uplink bridge: a normal (non-internal) bridge, from which the relay reaches the host.
pub const UPLINK: &str = "apz-mcp-uplink";
pub const UPLINK_SUBNET: &str = "172.29.251.0/24";
pub const UPLINK_GATEWAY: Ipv4Addr = Ipv4Addr::new(172, 29, 251, 1);
/// The relay's fixed address on `apz-mcp`: the proxy address the MCP containers are given.
pub const RELAY_LAN_IP: Ipv4Addr = Ipv4Addr::new(172, 29, 250, 2);
/// The relay container, its image and the path of the relay binary inside it.
pub const RELAY: &str = "apz-mcp-relay";
pub const RELAY_IMAGE: &str = "alpine:latest";
pub const RELAY_IN_CONTAINER: &str = "/usr/local/bin/mcp_relay";
/// The name the relay uses for the host, and the value that maps it on every engine.
pub const HOST_NAME: &str = "host.docker.internal";
pub const HOST_GATEWAY: &str = "host-gateway";
/// The environment variable that carries the session token (its value is never in a command line).
pub const TOKEN_ENV: &str = "APZ_RELAY_TOKEN";
/// How long the relay has to report that it listens.
const READY_WAIT: Duration = Duration::from_secs(10);

/// The musl target the relay is built for, for a Docker architecture.
fn musl_triple(arch: &str) -> &'static str {
    match arch {
        "aarch64" => "aarch64-unknown-linux-musl",
        _ => "x86_64-unknown-linux-musl",
    }
}

/// The architecture of the Docker engine the relay runs on: `x86_64` or `aarch64`. Docker Desktop's VM decides it
/// on macOS and Windows, so it is asked, not assumed.
pub fn docker_arch(docker: &dyn Docker) -> Result<&'static str, String> {
    let args: Vec<String> = ["info", "--format", "{{.Architecture}}"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let ran = docker.run(&args, &[]).map_err(|_| NO_DOCKER.to_string())?;
    if !ran.ok {
        return Err(format!(
            "docker info failed ({}); remote (http) servers still work",
            first_line(&ran.stderr)
        ));
    }
    match ran.stdout.trim() {
        "x86_64" | "amd64" => Ok("x86_64"),
        "aarch64" | "arm64" => Ok("aarch64"),
        other => Err(format!(
            "docker runs {other} containers, which the relay does not support; remote (http) servers still work"
        )),
    }
}

/// Where the relay binary is found for a Docker architecture: the musl build in a development tree
/// (`target/<musl>/release/mcp_relay`), then `APZ_MCP_RELAY`, then the bundle beside apid (`mcp_relay-<arch>`, or
/// `mcp_relay`).
pub fn find_source(arch: &str) -> PathBuf {
    let exe = std::env::current_exe().ok();
    if let Some(target) = exe
        .as_ref()
        .and_then(|e| e.parent())
        .and_then(|p| p.parent())
    {
        let dev = target
            .join(musl_triple(arch))
            .join("release")
            .join("mcp_relay");
        if dev.is_file() {
            return dev;
        }
    }
    if let Some(path) = std::env::var_os("APZ_MCP_RELAY").filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    match exe.and_then(|e| e.parent().map(Path::to_path_buf)) {
        Some(dir) => {
            let named = dir.join(format!("mcp_relay-{arch}"));
            if named.is_file() {
                named
            } else {
                dir.join("mcp_relay")
            }
        }
        None => PathBuf::from("mcp_relay"),
    }
}

/// Copies the relay binary into `<state>/mcp/mcp_relay` (executable), and returns that path. Docker Desktop shares
/// the home directory, so a copy there can be mounted where the source may not be.
pub fn install(source: &Path, state_dir: &Path) -> Result<PathBuf, String> {
    if !source.is_file() {
        return Err(format!(
            "the relay binary {} was not found; build it with `cargo build -p mcp-relay --release --target x86_64-unknown-linux-musl` (or aarch64-unknown-linux-musl); remote (http) servers still work",
            source.display()
        ));
    }
    let dir = state_dir.join("mcp");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let dest = dir.join("mcp_relay");
    let temp = dir.join("mcp_relay.tmp");
    std::fs::copy(source, &temp).map_err(|e| format!("cannot copy the relay binary: {e}"))?;
    set_executable(&temp)?;
    std::fs::rename(&temp, &dest).map_err(|e| format!("cannot install the relay binary: {e}"))?;
    Ok(dest)
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("cannot make the relay binary executable: {e}"))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_string()
}

fn strings(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

pub fn uplink_inspect_args() -> Vec<String> {
    strings(&[
        "network",
        "inspect",
        "-f",
        "{{.Internal}} {{range .IPAM.Config}}{{.Subnet}} {{.Gateway}}{{end}}",
        UPLINK,
    ])
}

pub fn uplink_create_args() -> Vec<String> {
    let gateway = UPLINK_GATEWAY.to_string();
    let mut args = strings(&[
        "network",
        "create",
        "--driver",
        "bridge",
        "--subnet",
        UPLINK_SUBNET,
        "--gateway",
    ]);
    args.push(gateway);
    args.push(UPLINK.into());
    args
}

/// Makes the uplink bridge if it is missing, and checks it is a normal bridge with its fixed subnet.
pub fn ensure_uplink(docker: &dyn Docker) -> Result<(), String> {
    let inspect = |docker: &dyn Docker| {
        docker
            .run(&uplink_inspect_args(), &[])
            .map_err(|_| NO_DOCKER.to_string())
    };
    let mut ran = inspect(docker)?;
    if !ran.ok {
        if ran.stderr.contains("Cannot connect to the Docker daemon") {
            return Err(NO_DAEMON.to_string());
        }
        let created = docker
            .run(&uplink_create_args(), &[])
            .map_err(|_| NO_DOCKER.to_string())?;
        if !created.ok {
            return Err(format!(
                "the {UPLINK} network could not be created ({}); remote (http) servers still work",
                first_line(&created.stderr)
            ));
        }
        ran = inspect(docker)?;
        if !ran.ok {
            return Err(format!(
                "the {UPLINK} network was created but cannot be found; remote (http) servers still work"
            ));
        }
    }
    let mut fields = ran.stdout.split_whitespace();
    let internal = fields.next() == Some("true");
    let subnet = fields.next();
    if internal {
        return Err(format!(
            "the {UPLINK} network is --internal, so the relay cannot reach the host; run `docker network rm {UPLINK}` and try again; remote (http) servers still work"
        ));
    }
    if subnet != Some(UPLINK_SUBNET) {
        return Err(format!(
            "the {UPLINK} network has subnet {}, apid needs {UPLINK_SUBNET}; run `docker network rm {UPLINK}` and try again; remote (http) servers still work",
            subnet.unwrap_or("none")
        ));
    }
    Ok(())
}

/// `docker create` for the relay: on `apz-mcp` at its fixed address, able to reach the host by name, with the
/// read-only binary, the hardened flags of a stdio server, and the token by name only. `--pull never`: apid never
/// pulls an image.
pub fn create_args(binary: &Path, port: u16) -> Vec<String> {
    let mut args = strings(&[
        "create",
        "--name",
        RELAY,
        "--pull",
        "never",
        "--network",
        NETWORK,
        "--ip",
    ]);
    args.push(RELAY_LAN_IP.to_string());
    args.push(format!("--add-host={HOST_NAME}:{HOST_GATEWAY}"));
    args.extend(strings(&[
        "--read-only",
        "--tmpfs",
        "/tmp:size=16m",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--user",
        "65534:65534",
        "--memory",
        "64m",
        "--pids-limit",
        "64",
        "-i",
        "-e",
        TOKEN_ENV,
    ]));
    args.push("-e".into());
    args.push(format!("APZ_RELAY_LISTEN={RELAY_LAN_IP}:3128"));
    args.push("-e".into());
    args.push(format!("APZ_RELAY_UPSTREAM={HOST_NAME}:{port}"));
    args.push("-v".into());
    args.push(format!("{}:{RELAY_IN_CONTAINER}:ro", binary.display()));
    args.push(RELAY_IMAGE.into());
    args.push(RELAY_IN_CONTAINER.into());
    args
}

/// `docker network connect` for the relay's second network: the uplink, which reaches the host.
pub fn connect_args() -> Vec<String> {
    strings(&["network", "connect", UPLINK, RELAY])
}

pub fn remove_args() -> Vec<String> {
    strings(&["rm", "-f", RELAY])
}

/// A running relay: its attached `docker start` process, whose stdin apid holds.
pub struct RelayProcess {
    docker: Arc<dyn Docker>,
    child: Child,
}

impl RelayProcess {
    /// Whether the relay is still running.
    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for RelayProcess {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = self.docker.run(&remove_args(), &[]);
    }
}

/// Starts the relay from the installed binary `binary`, with `token` in its environment, relaying to the host at
/// `port`. Its networks must exist already. The relay is up when it has reported `ready`; a relay that does not
/// report in time is removed and the start fails.
pub fn start(
    docker: Arc<dyn Docker>,
    binary: &Path,
    token: &str,
    port: u16,
) -> Result<RelayProcess, String> {
    if !binary.is_file() {
        return Err(format!(
            "the relay binary {} is missing; remote (http) servers still work",
            binary.display()
        ));
    }
    // A stale relay (stopped, or left by an apid that was killed) is removed before the new one is created.
    let _ = docker.run(&remove_args(), &[]);
    let created = docker
        .run(&create_args(binary, port), &[(TOKEN_ENV, token)])
        .map_err(|_| NO_DOCKER.to_string())?;
    if !created.ok {
        if created.stderr.contains("No such image") {
            return Err(format!(
                "the relay image {RELAY_IMAGE} is not on this machine; pull it yourself with `docker pull {RELAY_IMAGE}` (apid never pulls); remote (http) servers still work"
            ));
        }
        return Err(format!(
            "the relay container could not be created ({}); remote (http) servers still work",
            first_line(&created.stderr)
        ));
    }
    let joined = docker
        .run(&connect_args(), &[])
        .map_err(|_| NO_DOCKER.to_string())?;
    if !joined.ok {
        let _ = docker.run(&remove_args(), &[]);
        return Err(format!(
            "the relay could not join the {UPLINK} network ({}); remote (http) servers still work",
            first_line(&joined.stderr)
        ));
    }
    let child = docker
        .start_attached(RELAY)
        .map_err(|_| NO_DOCKER.to_string())?;
    // From here the handle owns the container: if the relay does not come up, dropping it removes the container.
    let mut process = RelayProcess { docker, child };
    let mut stdout = process
        .child
        .stdout
        .take()
        .ok_or("the relay has no output")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(&mut stdout).read_line(&mut line);
        let _ = tx.send(line);
    });
    match rx.recv_timeout(READY_WAIT) {
        Ok(line) if line.trim() == "ready" => {
            if !process.alive() {
                return Err("the relay exited at start; remote (http) servers still work".into());
            }
            Ok(process)
        }
        _ => Err("the relay did not report ready; remote (http) servers still work".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_uplink_is_a_normal_bridge_with_its_fixed_subnet_and_gateway() {
        assert_eq!(
            uplink_create_args().join(" "),
            "network create --driver bridge --subnet 172.29.251.0/24 --gateway 172.29.251.1 apz-mcp-uplink"
        );
        assert!(!uplink_create_args().iter().any(|a| a == "--internal"));
    }

    #[test]
    fn the_relay_sits_at_its_fixed_address_listens_there_and_joins_the_uplink() {
        let args = create_args(Path::new("/state/mcp/mcp_relay"), 19128);
        let at = args.iter().position(|a| a == "--ip").unwrap();
        assert_eq!(args[at + 1], "172.29.250.2");
        assert!(
            args.join(" ")
                .contains("APZ_RELAY_LISTEN=172.29.250.2:3128")
        );
        assert_eq!(
            connect_args().join(" "),
            "network connect apz-mcp-uplink apz-mcp-relay"
        );
    }

    #[test]
    fn the_relay_can_reach_the_host_by_name_on_every_engine() {
        let args = create_args(Path::new("/state/mcp/mcp_relay"), 19128);
        assert!(
            args.iter()
                .any(|a| a == "--add-host=host.docker.internal:host-gateway")
        );
        assert!(
            args.iter()
                .any(|a| a == "APZ_RELAY_UPSTREAM=host.docker.internal:19128")
        );
    }

    #[test]
    fn the_relay_runs_hardened_with_a_read_only_binary_and_never_pulls() {
        let joined = create_args(Path::new("/state/mcp/mcp_relay"), 19128).join(" ");
        for flag in [
            "--read-only",
            "--cap-drop ALL",
            "--security-opt no-new-privileges",
            "--user 65534:65534",
            "--pull never",
        ] {
            assert!(joined.contains(flag), "{flag} missing from {joined}");
        }
        assert!(joined.contains("-v /state/mcp/mcp_relay:/usr/local/bin/mcp_relay:ro"));
        assert!(joined.contains(" alpine:latest /usr/local/bin/mcp_relay"));
    }

    #[test]
    fn the_token_is_named_in_argv_and_never_valued() {
        let args = create_args(Path::new("/state/mcp/mcp_relay"), 19128);
        assert!(args.windows(2).any(|w| w[0] == "-e" && w[1] == TOKEN_ENV));
        assert!(
            !args
                .iter()
                .any(|a| a.contains("secret") || a.contains("token="))
        );
    }

    #[test]
    fn install_copies_the_binary_into_the_state_directory_executable() {
        let root = std::env::temp_dir().join(format!("apz-relay-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("built");
        std::fs::write(&source, b"relay-bytes").unwrap();
        let installed = install(&source, &root.join("state")).unwrap();
        assert_eq!(installed, root.join("state/mcp/mcp_relay"));
        assert_eq!(std::fs::read(&installed).unwrap(), b"relay-bytes");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&installed).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
        assert!(
            install(&root.join("missing"), &root.join("state"))
                .unwrap_err()
                .contains("cargo build -p mcp-relay")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_relay_follows_the_docker_architecture() {
        use crate::mcp::stdio::Ran;
        use crate::mcp::stdio::tests::MockDocker;
        let answer = |out: &'static str| {
            MockDocker::new(Default::default(), move |_: &[String]| {
                Ok(Ran {
                    ok: true,
                    stdout: out.into(),
                    stderr: String::new(),
                })
            })
        };
        assert_eq!(docker_arch(&answer("aarch64")).unwrap(), "aarch64");
        assert_eq!(docker_arch(&answer("x86_64")).unwrap(), "x86_64");
        assert!(
            docker_arch(&answer("riscv64"))
                .unwrap_err()
                .contains("does not support")
        );
    }
}
