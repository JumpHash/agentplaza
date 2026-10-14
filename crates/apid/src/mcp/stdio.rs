//! Stdio servers: a container (or, in tests, a plain child process) the hub keeps the pipes of.
//!
//! The docker launcher runs `docker run` with the hardened flags of docs/design/mcp.md. A secret reaches
//! the container as `-e NAME` with no value: the docker client reads the value from its own environment,
//! which apid sets for that child only. Values never appear in argv or on disk.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use std::net::Ipv4Addr;

use serde_json::Value;

use super::config::volume_name;
use super::egress::Lease;

use super::client::{Error, Transport};

/// The network a stdio server's container joins; its egress follows the hub's rules.
pub const NETWORK: &str = "apz-mcp";
/// The longest line a server may send (a larger one ends the connection).
const MAX_LINE: usize = 4 << 20;
/// Environment the docker client needs to find its daemon, passed through from apid.
const PASSTHROUGH: [&str; 5] = [
    "PATH",
    "HOME",
    "DOCKER_HOST",
    "DOCKER_CONFIG",
    "XDG_RUNTIME_DIR",
];

/// What a launch needs: the image, the command after it, and the env the child gets.
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchSpec {
    pub server: String,
    pub image: String,
    pub command: Vec<String>,
    pub workdir: Option<String>,
    /// Name and value of every env var the server gets (secrets and literals).
    pub env: Vec<(String, String)>,
    /// The container's address on the apz-mcp network (`--ip`), when the proxy is in use.
    pub ip: Option<Ipv4Addr>,
    /// The docker volume mounted at /data, when the server keeps storage.
    pub volume: Option<String>,
}

/// Starts a server's process and hands back the child whose pipes the hub keeps.
pub trait Launcher: Send + Sync {
    fn launch(&self, spec: &LaunchSpec) -> Result<Child, String>;
}

/// The docker argv for `spec`: the hardened flags, `-e NAME` for each env var, then the image and command.
/// No value appears here.
pub fn docker_args(spec: &LaunchSpec) -> Vec<String> {
    let mut args: Vec<String> = [
        "run",
        "--rm",
        "-i",
        "--init",
        "--read-only",
        "--tmpfs",
        "/tmp:size=64m",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--user",
        "65534:65534",
        "--memory",
        "512m",
        "--cpus",
        "1",
        "--pids-limit",
        "256",
        "--network",
        NETWORK,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(ip) = spec.ip {
        args.push("--ip".into());
        args.push(ip.to_string());
    }
    if let Some(volume) = &spec.volume {
        args.push("--mount".into());
        args.push(format!("type=volume,source={volume},target=/data"));
    }
    if let Some(dir) = &spec.workdir {
        args.push("-w".into());
        args.push(dir.clone());
    }
    for (name, _) in &spec.env {
        args.push("-e".into());
        args.push(name.clone());
    }
    args.push(spec.image.clone());
    args.extend(spec.command.iter().cloned());
    args
}

/// The environment the docker client runs with: the passthrough variables and the server's env, nothing else.
pub fn child_env(spec: &LaunchSpec) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = PASSTHROUGH
        .iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| (name.to_string(), value))
        })
        .collect();
    env.extend(spec.env.iter().cloned());
    env
}

/// The subnet of the apz-mcp network, and its gateway: the address the egress proxy listens on.
pub const SUBNET: &str = "172.29.250.0/24";
pub const GATEWAY: Ipv4Addr = Ipv4Addr::new(172, 29, 250, 1);

/// `docker network create` for the apz-mcp network: internal, so it has no route out.
pub fn network_create_args() -> Vec<String> {
    [
        "network",
        "create",
        "--internal",
        "--driver",
        "bridge",
        "--subnet",
        SUBNET,
        "--gateway",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain([GATEWAY.to_string(), NETWORK.to_string()])
    .collect()
}

/// `docker network inspect` for the apz-mcp network: "<internal> <subnet> <gateway>".
pub fn network_inspect_args() -> Vec<String> {
    [
        "network",
        "inspect",
        "-f",
        "{{.Internal}} {{range .IPAM.Config}}{{.Subnet}} {{.Gateway}}{{end}}",
        NETWORK,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// `docker volume rm` for a server's storage volume.
pub fn volume_remove_args(server: &str) -> Vec<String> {
    vec![
        "volume".into(),
        "rm".into(),
        "-f".into(),
        volume_name(server),
    ]
}

/// What apid says when docker cannot run: the container servers are stopped, the remote ones still work.
pub const NO_DOCKER: &str = "docker is not available: install Docker to run container MCP servers; remote (http) servers still work";
/// What apid says when the docker client runs but its daemon is not reachable.
pub const NO_DAEMON: &str = "docker is installed but its daemon is not reachable: start Docker to run container MCP servers; remote (http) servers still work";

/// The docker client as apid runs it. Tests replace it with a mock that answers from a script.
pub trait Docker: Send + Sync {
    /// Runs `docker <args>` with `env` added to its environment (the values are never in `args`). An `Err` means
    /// the client itself could not run (docker is not installed).
    fn run(&self, args: &[String], env: &[(&str, &str)]) -> std::io::Result<Ran>;
    /// Runs `docker start -a -i <name>` as a child whose stdin apid holds: the container runs while it is open.
    fn start_attached(&self, name: &str) -> std::io::Result<Child>;
}

/// The outcome of one docker command.
#[derive(Debug, Clone, Default)]
pub struct Ran {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// The real docker client.
pub struct SystemDocker;

impl Docker for SystemDocker {
    fn start_attached(&self, name: &str) -> std::io::Result<Child> {
        Command::new("docker")
            .args(["start", "-a", "-i", name])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
    }

    fn run(&self, args: &[String], env: &[(&str, &str)]) -> std::io::Result<Ran> {
        let out = Command::new("docker")
            .args(args)
            .envs(env.iter().copied())
            .output()?;
        Ok(Ran {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

fn daemon_down(stderr: &str) -> bool {
    stderr.contains("Cannot connect to the Docker daemon")
        || stderr.contains("Is the docker daemon running")
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_string()
}

/// What `docker network inspect` found: the network's line, or nothing.
enum Found {
    Line(String),
    Absent,
}

fn inspect(docker: &dyn Docker) -> Result<Found, String> {
    let ran = docker
        .run(&network_inspect_args(), &[])
        .map_err(|_| NO_DOCKER.to_string())?;
    if ran.ok {
        Ok(Found::Line(ran.stdout.trim().to_string()))
    } else if daemon_down(&ran.stderr) {
        Err(NO_DAEMON.to_string())
    } else {
        Ok(Found::Absent)
    }
}

/// Makes the apz-mcp network if it is missing, then checks it is internal with the fixed subnet. Returns the
/// gateway address. The gateway is not yet checked on the host: the proxy binds it only after this succeeds
/// (see `egress::start_gateway`).
pub fn ensure_network(docker: &dyn Docker) -> Result<Ipv4Addr, String> {
    let line = match inspect(docker)? {
        Found::Line(line) => line,
        Found::Absent => {
            let created = docker
                .run(&network_create_args(), &[])
                .map_err(|_| NO_DOCKER.to_string())?;
            if !created.ok {
                return Err(if daemon_down(&created.stderr) {
                    NO_DAEMON.to_string()
                } else {
                    format!(
                        "the {NETWORK} network could not be created ({}); remote (http) servers still work",
                        first_line(&created.stderr)
                    )
                });
            }
            match inspect(docker)? {
                Found::Line(line) => line,
                Found::Absent => {
                    return Err(format!(
                        "the {NETWORK} network was created but cannot be found; remote (http) servers still work"
                    ));
                }
            }
        }
    };
    let mut fields = line.split_whitespace();
    let internal = fields.next() == Some("true");
    let subnet = fields.next();
    let gateway = fields.next().and_then(|g| g.parse::<Ipv4Addr>().ok());
    if !internal {
        return Err(format!(
            "the {NETWORK} network exists but is not --internal; run `docker network rm {NETWORK}` and try again; remote (http) servers still work"
        ));
    }
    if subnet != Some(SUBNET) {
        return Err(format!(
            "the {NETWORK} network has subnet {}, apid needs {SUBNET}; run `docker network rm {NETWORK}` and try again; remote (http) servers still work",
            subnet.unwrap_or("none")
        ));
    }
    gateway.ok_or_else(|| format!("the {NETWORK} network has no gateway; run `docker network rm {NETWORK}` and try again; remote (http) servers still work"))
}

/// Removes a server's storage volume (`DELETE /v1/mcp/servers/{id}?purge=1`).
pub fn purge_volume(server: &str) -> Result<(), String> {
    let ran = SystemDocker
        .run(&volume_remove_args(server), &[])
        .map_err(|_| NO_DOCKER.to_string())?;
    if ran.ok {
        Ok(())
    } else {
        Err(first_line(&ran.stderr))
    }
}

/// Runs the server in a container through the `docker` client.
pub struct DockerLauncher;

impl Launcher for DockerLauncher {
    fn launch(&self, spec: &LaunchSpec) -> Result<Child, String> {
        Command::new("docker")
            .args(docker_args(spec))
            .env_clear()
            .envs(child_env(spec))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot start docker: {e}"))
    }
}

/// Runs `spec.image` as a plain process with `spec.command` as its arguments. Tests only: no container.
#[cfg(test)]
pub struct PlainLauncher;

#[cfg(test)]
impl Launcher for PlainLauncher {
    fn launch(&self, spec: &LaunchSpec) -> Result<Child, String> {
        // Under the target's runner when cargo has one (an arm64 fixture on the x86_64 CI host).
        apz_os::runner::command(std::path::Path::new(&spec.image))
            .args(&spec.command)
            .env_clear()
            .envs(spec.env.iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", spec.image))
    }
}

/// Reads one line of at most `cap` bytes. `Ok(None)` at the end of the stream.
pub(crate) fn read_line_capped(
    reader: &mut impl BufRead,
    cap: usize,
) -> std::io::Result<Option<String>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok((!line.is_empty()).then(|| String::from_utf8_lossy(&line).into_owned()));
        }
        match available.iter().position(|b| *b == b'\n') {
            Some(at) => {
                line.extend_from_slice(&available[..at]);
                reader.consume(at + 1);
                return Ok(Some(String::from_utf8_lossy(&line).into_owned()));
            }
            None => {
                let take = available.len();
                line.extend_from_slice(available);
                reader.consume(take);
                if line.len() > cap {
                    return Err(std::io::Error::other("a line over the size cap"));
                }
            }
        }
    }
}

/// The stdio transport: writes JSON lines to the child's stdin, reads its stdout on a thread, and gives
/// each stderr line to `on_stderr`.
pub struct StdioTransport {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<Result<String, ()>>,
    closed: bool,
    /// The container's address, freed when the transport drops.
    _lease: Option<Lease>,
}

impl StdioTransport {
    /// Takes over a started child. `on_stderr` gets every stderr line, as the child wrote it.
    pub fn new(
        mut child: Child,
        on_stderr: Arc<dyn Fn(String) + Send + Sync>,
        lease: Option<Lease>,
    ) -> Result<StdioTransport, String> {
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr = child.stderr.take().ok_or("no stderr")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match read_line_capped(&mut reader, MAX_LINE) {
                    Ok(Some(line)) => {
                        if tx.send(Ok(line)).is_err() {
                            return;
                        }
                    }
                    Ok(None) | Err(_) => {
                        let _ = tx.send(Err(()));
                        return;
                    }
                }
            }
        });
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            while let Ok(Some(line)) = read_line_capped(&mut reader, 64 << 10) {
                on_stderr(line);
            }
        });
        Ok(StdioTransport {
            child,
            stdin: Some(stdin),
            lines: rx,
            closed: false,
            _lease: lease,
        })
    }
}

impl Transport for StdioTransport {
    fn send(&mut self, message: &Value, timeout: Duration) -> Result<Vec<Value>, Error> {
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(Error::Transport("the server is not running".into()));
        };
        let mut line =
            serde_json::to_string(message).map_err(|e| Error::Protocol(e.to_string()))?;
        line.push('\n');
        if stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .is_err()
        {
            return Err(Error::Transport(
                "the server stopped reading its input".into(),
            ));
        }
        let Some(id) = message.get("id").cloned() else {
            return Ok(Vec::new());
        };
        let deadline = Instant::now() + timeout;
        let mut seen = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(Ok(text)) => {
                    // Anything that is not JSON-RPC on stdout is noise from the server; it is skipped.
                    let Ok(value) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    let is_reply = value.get("id") == Some(&id) && value.get("method").is_none();
                    seen.push(value);
                    if is_reply {
                        return Ok(seen);
                    }
                }
                Ok(Err(())) => return Err(Error::Transport("the server exited".into())),
                Err(RecvTimeoutError::Timeout) => return Err(Error::Timeout),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Error::Transport("the server exited".into()));
                }
            }
        }
    }

    fn alive(&mut self) -> bool {
        !self.closed && matches!(self.child.try_wait(), Ok(None))
    }

    fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.stdin = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        self.close();
    }
}

/// The last lines a server wrote on stderr, oldest first, capped at `cap`.
pub fn push_capped<T>(lines: &mut VecDeque<T>, item: T, cap: usize) {
    if lines.len() >= cap {
        lines.pop_front();
    }
    lines.push_back(item);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn spec() -> LaunchSpec {
        LaunchSpec {
            server: "github".into(),
            image: "ghcr.io/github/github-mcp-server".into(),
            command: vec!["stdio".into()],
            workdir: None,
            env: vec![(
                "GITHUB_PERSONAL_ACCESS_TOKEN".into(),
                "SENTINEL-TOKEN-7731".into(),
            )],
            ip: None,
            volume: None,
        }
    }

    #[test]
    fn docker_argv_has_the_hardened_flags_in_order() {
        let args = docker_args(&spec());
        let joined = args.join(" ");
        assert!(joined.starts_with(
            "run --rm -i --init --read-only --tmpfs /tmp:size=64m --cap-drop ALL --security-opt no-new-privileges \
             --user 65534:65534 --memory 512m --cpus 1 --pids-limit 256 --network apz-mcp"
        ));
        assert!(joined.ends_with("ghcr.io/github/github-mcp-server stdio"));
    }

    #[test]
    fn secrets_are_named_in_argv_and_never_valued() {
        let args = docker_args(&spec());
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-e" && w[1] == "GITHUB_PERSONAL_ACCESS_TOKEN")
        );
        assert!(!args.iter().any(|a| a.contains("SENTINEL")));
    }

    #[test]
    fn the_child_env_carries_the_value_and_no_other_apid_variable() {
        let env = child_env(&spec());
        assert!(
            env.iter()
                .any(|(k, v)| k == "GITHUB_PERSONAL_ACCESS_TOKEN" && v == "SENTINEL-TOKEN-7731")
        );
        assert!(
            env.iter()
                .all(|(k, _)| PASSTHROUGH.contains(&k.as_str())
                    || k == "GITHUB_PERSONAL_ACCESS_TOKEN")
        );
    }

    #[test]
    fn workdir_becomes_dash_w_before_the_image() {
        let mut with_dir = spec();
        with_dir.workdir = Some("/workspace".into());
        let args = docker_args(&with_dir);
        let at = args.iter().position(|a| a == "-w").unwrap();
        assert_eq!(args[at + 1], "/workspace");
        assert!(
            at < args
                .iter()
                .position(|a| a == "ghcr.io/github/github-mcp-server")
                .unwrap()
        );
    }

    #[test]
    fn a_capped_line_reader_splits_lines_and_refuses_an_endless_one() {
        let mut reader = std::io::Cursor::new(b"one\ntwo\n".to_vec());
        assert_eq!(
            read_line_capped(&mut reader, 64).unwrap().as_deref(),
            Some("one")
        );
        assert_eq!(
            read_line_capped(&mut reader, 64).unwrap().as_deref(),
            Some("two")
        );
        assert_eq!(read_line_capped(&mut reader, 64).unwrap(), None);
        let mut endless = std::io::Cursor::new(vec![b'x'; 100]);
        assert!(read_line_capped(&mut endless, 10).is_err());
    }

    #[test]
    fn push_capped_keeps_the_newest() {
        let mut lines = VecDeque::new();
        for i in 0..5 {
            push_capped(&mut lines, i, 3);
        }
        assert_eq!(lines.into_iter().collect::<Vec<_>>(), vec![2, 3, 4]);
    }

    #[test]
    fn the_network_is_internal_and_has_the_fixed_subnet_and_gateway() {
        let create = network_create_args().join(" ");
        assert_eq!(
            create,
            "network create --internal --driver bridge --subnet 172.29.250.0/24 --gateway 172.29.250.1 apz-mcp"
        );
        assert!(network_inspect_args().join(" ").ends_with(" apz-mcp"));
    }

    #[test]
    fn a_container_gets_its_address_and_its_volume_and_no_host_path() {
        let mut with = spec();
        with.ip = Some(Ipv4Addr::new(172, 29, 250, 7));
        with.volume = Some("apz-mcp-memory".into());
        let args = docker_args(&with);
        let at = args.iter().position(|a| a == "--ip").unwrap();
        assert_eq!(args[at + 1], "172.29.250.7");
        assert!(
            args.iter()
                .any(|a| a == "type=volume,source=apz-mcp-memory,target=/data")
        );
        assert!(
            !args
                .iter()
                .any(|a| a.starts_with("/") && a != "/tmp:size=64m"),
            "{args:?}"
        );
    }

    #[test]
    fn the_purge_removes_the_named_volume_only() {
        assert_eq!(
            volume_remove_args("memory"),
            ["volume", "rm", "-f", "apz-mcp-memory"]
        );
    }

    fn ran(ok: bool, stdout: &str, stderr: &str) -> std::io::Result<Ran> {
        Ok(Ran {
            ok,
            stdout: stdout.into(),
            stderr: stderr.into(),
        })
    }

    #[test]
    fn a_missing_docker_gives_one_actionable_line() {
        let docker = MockDocker::new(Default::default(), |_: &[String]| {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        });
        assert_eq!(ensure_network(&docker).unwrap_err(), NO_DOCKER);
        assert!(
            NO_DOCKER.contains("install Docker")
                && NO_DOCKER.contains("remote (http) servers still work")
        );
    }

    #[test]
    fn an_unreachable_daemon_says_so() {
        let docker = MockDocker::new(Default::default(), |_: &[String]| {
            ran(
                false,
                "",
                "Cannot connect to the Docker daemon at unix:///var/run/docker.sock.",
            )
        });
        assert_eq!(ensure_network(&docker).unwrap_err(), NO_DAEMON);
    }

    #[test]
    fn a_network_that_cannot_be_created_names_dockers_reason_and_no_socket_error() {
        let docker = MockDocker::new(Default::default(), |args: &[String]| {
            if args.get(1).map(String::as_str) == Some("create") {
                ran(
                    false,
                    "",
                    "Error response from daemon: permission denied while trying to connect\nmore",
                )
            } else {
                ran(false, "", "Error: No such network: apz-mcp")
            }
        });
        let err = ensure_network(&docker).unwrap_err();
        assert!(
            err.contains("could not be created") && err.contains("permission denied"),
            "{err}"
        );
        assert!(!err.contains("os error") && !err.contains("\n"), "{err}");
    }

    #[test]
    fn an_existing_network_must_be_internal_with_the_fixed_subnet() {
        let not_internal = MockDocker::new(Default::default(), |_: &[String]| {
            ran(true, "false 172.29.250.0/24 172.29.250.1", "")
        });
        assert!(
            ensure_network(&not_internal)
                .unwrap_err()
                .contains("not --internal")
        );
        let other_subnet = MockDocker::new(Default::default(), |_: &[String]| {
            ran(true, "true 10.9.0.0/24 10.9.0.1", "")
        });
        assert!(
            ensure_network(&other_subnet)
                .unwrap_err()
                .contains("subnet 10.9.0.0/24")
        );
    }

    #[test]
    fn a_created_network_is_inspected_again_before_its_gateway_is_used() {
        let created = Arc::new(Mutex::new(false));
        let log = Arc::new(Mutex::new(Vec::new()));
        let flag = Arc::clone(&created);
        let docker = MockDocker::new(Arc::clone(&log), move |args: &[String]| {
            if args.get(1).map(String::as_str) == Some("create") {
                *flag.lock().unwrap() = true;
                ran(true, "apz-mcp", "")
            } else if *flag.lock().unwrap() {
                ran(true, "true 172.29.250.0/24 172.29.250.1", "")
            } else {
                ran(false, "", "Error: No such network: apz-mcp")
            }
        });
        assert_eq!(ensure_network(&docker).unwrap(), GATEWAY);
        let calls = log.lock().unwrap().clone();
        assert_eq!(calls.len(), 3, "{calls:?}");
        assert!(
            calls[0].starts_with("network inspect")
                && calls[1].starts_with("network create")
                && calls[2].starts_with("network inspect")
        );
    }

    /// A docker client that answers each command through a closure, and logs each command line. Starting the
    /// relay runs a stand-in child that reports `ready` and then waits on its stdin.
    pub(crate) struct MockDocker {
        pub(crate) log: Arc<Mutex<Vec<String>>>,
        answer: Box<dyn Fn(&[String]) -> std::io::Result<Ran> + Send + Sync>,
    }

    impl MockDocker {
        pub(crate) fn new(
            log: Arc<Mutex<Vec<String>>>,
            answer: impl Fn(&[String]) -> std::io::Result<Ran> + Send + Sync + 'static,
        ) -> MockDocker {
            MockDocker {
                log,
                answer: Box::new(answer),
            }
        }

        /// Every command succeeds, and nothing is logged anywhere that matters.
        pub(crate) fn silent() -> MockDocker {
            MockDocker::new(Default::default(), |_: &[String]| {
                Ok(Ran {
                    ok: true,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            })
        }
    }

    impl Docker for MockDocker {
        fn run(&self, args: &[String], _env: &[(&str, &str)]) -> std::io::Result<Ran> {
            self.log.lock().unwrap().push(args.join(" "));
            (self.answer)(args)
        }

        fn start_attached(&self, name: &str) -> std::io::Result<Child> {
            self.log.lock().unwrap().push(format!("start -a -i {name}"));
            Command::new("sh")
                .args(["-c", "echo ready; exec cat"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
        }
    }
}
