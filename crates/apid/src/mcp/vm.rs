//! Stdio servers in microVMs: where the local node can boot one, a server runs in a microVM instead of under the OS
//! sandbox (docs/design/mcp.md, "microVMs").
//!
//! The hub asks the node (`noded`) to boot a VM with the server's directories as read-only volumes and its own
//! directory as the writable `/workspace`. The guest bridge (`vsockd`'s `mcp-bridge`) runs the server with its stdio
//! on vsock, and the guest's only route out is the hub's egress proxy, through the node. The hub's side of the stdio
//! is two Unix sockets the node opens for the run, and its end is the node's run state.
//!
//! The sandbox spec a server would run with is translated for the guest by [`request`]: every path in it is either
//! under a volume (`/data/<name>`) or under the server's own directory (`/workspace`). A path in neither is refused.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

use apz_sandbox::Spec;
use apz_uds::UnixStream;
use serde::Serialize;
use serde_json::Value;

use super::runtimes::sha256_hex;
use super::stdio::Child;
use crate::upstream::Upstream;

/// The port the guest bridge's HTTP proxy listens on inside the guest. Every server's proxy variables name it.
pub const GUEST_PROXY_PORT: u16 = 3128;
/// Where the guest mounts the server's writable directory: the server's own directory on the hub.
const GUEST_WORKSPACE: &str = "/workspace";
/// Where the guest mounts a volume: `/data/<name>`.
const GUEST_DATA: &str = "/data";
/// How often a wait asks the node whether the run has ended.
const POLL: Duration = Duration::from_millis(50);

/// Starts a server's process in a microVM. The child's stdio is the run's sockets.
pub trait VmHost: Send + Sync {
    /// Whether this node can run a server in a microVM now. The error says why not.
    fn available(&self) -> Result<(), String>;
    /// Boots the request's VM and returns the child that runs the server in it.
    fn launch(&self, request: &VmRequest) -> Result<Box<dyn Child>, String>;
    /// Removes the storage a server keeps between its runs.
    fn purge(&self, server: &str) -> Result<(), String>;
}

/// A read-only volume the guest mounts: `source` is the directory the node copies into the volume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VmMount {
    pub name: String,
    pub source: String,
}

/// The `POST /v1/mcp-runs` body (docs/design/mcp.md, "microVMs"). `env` holds the values the server gets, secrets
/// included: they reach the node in this request and nowhere else.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VmRequest {
    pub id: String,
    pub server: String,
    pub program: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: String,
    pub storage: bool,
    pub egress_port: u16,
    pub mounts: Vec<VmMount>,
}

/// The node's microVM service, reached over its HTTP API.
pub struct VmLauncher {
    noded: Arc<Upstream>,
}

impl VmLauncher {
    pub fn new(noded: Upstream) -> VmLauncher {
        VmLauncher {
            noded: Arc::new(noded),
        }
    }
}

impl VmHost for VmLauncher {
    fn available(&self) -> Result<(), String> {
        let (status, body) = self.noded.call("GET", "/v1/mcp-vm", None)?;
        if status != 200 {
            return Err(error_text(status, &body));
        }
        if body.get("available").and_then(Value::as_bool) == Some(true) {
            return Ok(());
        }
        Err(body
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("this node cannot run microVMs")
            .to_string())
    }

    fn launch(&self, request: &VmRequest) -> Result<Box<dyn Child>, String> {
        let body = serde_json::to_value(request).map_err(|e| e.to_string())?;
        let (status, reply) = self.noded.call("POST", "/v1/mcp-runs", Some(&body))?;
        if status != 201 {
            return Err(error_text(status, &reply));
        }
        let run = Run {
            id: text_of(&reply, "id")?,
            stdio: text_of(&reply, "stdio")?,
            stderr: text_of(&reply, "stderr")?,
        };
        match VmChild::connect(Arc::clone(&self.noded), &run) {
            Ok(child) => Ok(Box::new(child)),
            Err(why) => {
                let _ = self
                    .noded
                    .call("DELETE", &format!("/v1/mcp-runs/{}", run.id), None);
                Err(why)
            }
        }
    }

    fn purge(&self, server: &str) -> Result<(), String> {
        let (status, reply) =
            self.noded
                .call("DELETE", &format!("/v1/mcp-storage/{server}"), None)?;
        if status == 200 {
            return Ok(());
        }
        Err(error_text(status, &reply))
    }
}

/// The run the node opened: its id, and the two sockets the hub reads and writes.
struct Run {
    id: String,
    stdio: String,
    stderr: String,
}

/// A run as a child: stdin and stdout are the stdio socket, stderr the stderr socket. Its end is the node's.
struct VmChild {
    id: String,
    noded: Arc<Upstream>,
    stdio: UnixStream,
    stderr: UnixStream,
}

impl VmChild {
    fn connect(noded: Arc<Upstream>, run: &Run) -> Result<VmChild, String> {
        let stdio = UnixStream::connect(&run.stdio)
            .map_err(|e| format!("cannot reach the run's stdio: {e}"))?;
        let stderr = UnixStream::connect(&run.stderr)
            .map_err(|e| format!("cannot reach the run's stderr: {e}"))?;
        Ok(VmChild {
            id: run.id.clone(),
            noded,
            stdio,
            stderr,
        })
    }
}

impl Child for VmChild {
    fn stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        let stdio = self.stdio.try_clone().ok()?;
        Some(Box::new(stdio))
    }

    fn stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        let stdio = self.stdio.try_clone().ok()?;
        Some(Box::new(stdio))
    }

    fn stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        let stderr = self.stderr.try_clone().ok()?;
        Some(Box::new(stderr))
    }

    fn try_wait(&mut self) -> io::Result<bool> {
        let path = format!("/v1/mcp-runs/{}", self.id);
        let (status, body) = self
            .noded
            .call("GET", &path, None)
            .map_err(io::Error::other)?;
        match status {
            // A run the node no longer has has ended.
            404 => Ok(true),
            200 => Ok(body.get("state").and_then(Value::as_str) == Some("exited")),
            _ => Err(io::Error::other(error_text(status, &body))),
        }
    }

    fn kill(&mut self) -> io::Result<()> {
        let path = format!("/v1/mcp-runs/{}", self.id);
        let (status, body) = self
            .noded
            .call("DELETE", &path, None)
            .map_err(io::Error::other)?;
        match status {
            200 | 404 => Ok(()),
            _ => Err(io::Error::other(error_text(status, &body))),
        }
    }

    fn wait(&mut self) -> io::Result<()> {
        while !self.try_wait()? {
            thread::sleep(POLL);
        }
        Ok(())
    }
}

/// The run id for a server's next start: unique on this hub, and within the node's id rules.
pub fn run_id(server: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    format!("{server}-{}-{n}", apz_plaza::now_ms())
}

/// The request that runs `spec` in a VM. Each read-only directory of the spec becomes a volume; the server's own
/// directory `root` is the guest's `/workspace`; the proxy variables name the guest's proxy and nothing else.
pub fn request(
    spec: &Spec,
    root: &Path,
    server: &str,
    id: &str,
    storage: bool,
    egress_port: u16,
) -> Result<VmRequest, String> {
    let mounts: Vec<(PathBuf, String)> = spec
        .read_only
        .iter()
        .filter(|dir| dir.is_dir())
        .map(|dir| (dir.clone(), volume_name(dir)))
        .collect();
    let guest = Guest { mounts, root };
    let program = guest.path(&spec.program).ok_or_else(|| {
        format!(
            "{} is in no directory the microVM mounts",
            spec.program.display()
        )
    })?;
    let args = spec
        .args
        .iter()
        .map(|arg| guest.text(arg))
        .collect::<Vec<_>>();
    let mut env = BTreeMap::new();
    for (key, value) in &spec.env {
        let value = if key.to_ascii_uppercase().contains("PROXY") && value.starts_with("http") {
            guest_proxy(value)
        } else if key.starts_with("TMP") {
            "/tmp".to_string()
        } else if key == "PATH" {
            value
                .split(':')
                .map(|dir| guest.text(dir))
                .collect::<Vec<_>>()
                .join(":")
        } else {
            guest.text(value)
        };
        env.insert(key.clone(), value);
    }
    Ok(VmRequest {
        id: id.to_string(),
        server: server.to_string(),
        program,
        args,
        env,
        cwd: GUEST_WORKSPACE.to_string(),
        storage,
        egress_port,
        mounts: guest
            .mounts
            .iter()
            .map(|(dir, name)| VmMount {
                name: name.clone(),
                source: dir.to_string_lossy().into_owned(),
            })
            .collect(),
    })
}

/// The volume name of a read-only directory: `ro-` and 12 hex digits of its path and its modification time, so a
/// directory that changed gets a new volume and an unchanged one keeps its.
fn volume_name(dir: &Path) -> String {
    let stamp = std::fs::metadata(dir)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |time| time.as_nanos());
    let digest = sha256_hex(format!("{}\n{stamp}", dir.display()).as_bytes());
    format!("ro-{}", &digest[..12])
}

/// The guest's view of the hub's paths.
struct Guest<'a> {
    mounts: Vec<(PathBuf, String)>,
    root: &'a Path,
}

impl Guest<'_> {
    /// Where the guest sees `path`, if it is in a volume or in the workspace. The longest volume wins.
    fn path(&self, path: &Path) -> Option<String> {
        let volume = self
            .mounts
            .iter()
            .filter(|(dir, _)| path.starts_with(dir))
            .max_by_key(|(dir, _)| dir.components().count());
        if let Some((dir, name)) = volume {
            let rest = path.strip_prefix(dir).ok()?;
            return Some(join(&format!("{GUEST_DATA}/{name}"), rest));
        }
        let rest = path.strip_prefix(self.root).ok()?;
        Some(join(GUEST_WORKSPACE, rest))
    }

    /// `text` with a path inside it translated; any other text as it is.
    fn text(&self, text: &str) -> String {
        let path = Path::new(text);
        if path.is_absolute() {
            self.path(path).unwrap_or_else(|| text.to_string())
        } else {
            text.to_string()
        }
    }
}

/// `base` with `rest` appended (as a guest path, with `/`).
fn join(base: &str, rest: &Path) -> String {
    let rest = rest.to_string_lossy();
    if rest.is_empty() {
        base.to_string()
    } else {
        format!("{base}/{rest}")
    }
}

/// The hub's proxy URL with its port named the guest's: `http://apz:<token>@127.0.0.1:19128` becomes
/// `http://apz:<token>@127.0.0.1:3128`.
fn guest_proxy(url: &str) -> String {
    match url.rsplit_once(':') {
        Some((head, _)) => format!("{head}:{GUEST_PROXY_PORT}"),
        None => url.to_string(),
    }
}

/// The text of a node's error reply, or the status when it has none.
fn error_text(status: u16, body: &Value) -> String {
    match body.get("error").and_then(Value::as_str) {
        Some(message) => message.to_string(),
        None => format!("the node answered {status}"),
    }
}

/// A string field of a node's reply.
fn text_of(body: &Value, field: &str) -> Result<String, String> {
    body.get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("the node's reply has no {field}"))
}

// Unix sockets stand in for the node's VM sockets, so the tests run where the microVM path does.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::net::TcpListener;
    use std::os::unix::net::UnixListener;
    use std::sync::Mutex;

    /// A fresh directory under the temp dir, named for the test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apz-vm-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The server's spec, shaped as an npm server's: its runtime and its package read-only, its own directory
    /// writable.
    fn npm_spec(base: &Path) -> Spec {
        let runtimes = base.join("runtimes");
        let pkg = base.join("pkg");
        let node = runtimes.join("node/bin/node");
        std::fs::create_dir_all(node.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(&node, b"").unwrap();
        Spec {
            program: node,
            args: vec![
                pkg.join("bin.js").to_string_lossy().into_owned(),
                "--x".into(),
            ],
            env: vec![
                (
                    "PATH".into(),
                    format!("{}/node/bin:/usr/bin", runtimes.display()),
                ),
                (
                    "HOME".into(),
                    base.join("home").to_string_lossy().into_owned(),
                ),
                (
                    "TMPDIR".into(),
                    base.join("tmp").to_string_lossy().into_owned(),
                ),
                (
                    "HTTPS_PROXY".into(),
                    "http://apz:TOKEN@127.0.0.1:19128".into(),
                ),
                ("NO_PROXY".into(), "localhost".into()),
            ],
            workdir: base.to_path_buf(),
            read_only: vec![runtimes, pkg],
            read_write: vec![base.join("home")],
            network: apz_sandbox::Network::LoopbackPort(19128),
            limits: apz_sandbox::Limits::default(),
        }
    }

    #[test]
    fn a_spec_becomes_a_request_with_volumes_a_workspace_and_the_guest_proxy() {
        let base = scratch("request");
        let spec = npm_spec(&base);
        let root = base.clone();
        let req = request(&spec, &root, "fetch", "fetch-1", true, 19128).unwrap();
        assert_eq!(req.server, "fetch");
        assert_eq!(req.cwd, GUEST_WORKSPACE);
        assert!(req.storage);
        assert_eq!(req.egress_port, 19128);
        assert_eq!(req.mounts.len(), 2);
        let volume_of = |dir: &str| {
            req.mounts
                .iter()
                .find(|m| m.source.ends_with(dir))
                .unwrap()
                .name
                .clone()
        };
        let rt = volume_of("runtimes");
        let pkg = volume_of("pkg");
        assert!(rt.starts_with("ro-") && rt.len() == 15, "{rt}");
        assert!(
            req.program
                .starts_with(&format!("/data/{rt}/node/bin/node"))
        );
        assert_eq!(req.args[0], format!("/data/{pkg}/bin.js"));
        assert_eq!(req.args[1], "--x");
        assert_eq!(req.env["HOME"], "/workspace/home");
        assert_eq!(req.env["TMPDIR"], "/tmp");
        assert_eq!(req.env["PATH"], format!("/data/{rt}/node/bin:/usr/bin"));
        assert_eq!(req.env["HTTPS_PROXY"], "http://apz:TOKEN@127.0.0.1:3128");
        assert_eq!(req.env["NO_PROXY"], "localhost");
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_path_in_no_mount_or_workspace_is_refused_and_a_name_changes_with_its_directory() {
        let base = scratch("refuse");
        let mut spec = npm_spec(&base);
        spec.program = PathBuf::from("/usr/bin/node");
        let refused = request(&spec, &base, "fetch", "fetch-1", false, 1).unwrap_err();
        assert!(refused.contains("/usr/bin/node"), "{refused}");
        let before = volume_name(&base.join("pkg"));
        std::fs::write(base.join("pkg/changed"), b"x").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::remove_file(base.join("pkg/changed")).unwrap();
        assert_ne!(before, volume_name(&base.join("pkg")));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_guest_proxy_names_the_guest_port_and_keeps_the_token() {
        assert_eq!(
            guest_proxy("http://apz:abc@127.0.0.1:19128"),
            "http://apz:abc@127.0.0.1:3128"
        );
    }

    /// A stand-in for the node's HTTP API: each request is answered by `route` (method, path, body) with a status and
    /// a JSON body. Requests are recorded in `seen`, as `METHOD path body`.
    fn fake_noded<F>(route: F, seen: Arc<Mutex<Vec<String>>>) -> u16
    where
        F: Fn(&str, &str, &str) -> (u16, String) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = io::BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut parts = line.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let path = parts.next().unwrap_or("").to_string();
                let mut length = 0usize;
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0u8; length];
                reader.read_exact(&mut body).unwrap();
                let body = String::from_utf8_lossy(&body).into_owned();
                seen.lock().unwrap().push(format!("{method} {path} {body}"));
                let (status, reply) = route(&method, &path, &body);
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        port
    }

    #[test]
    fn a_vm_run_is_launched_piped_and_stopped_through_the_node() {
        let dir = scratch("launch");
        let stdio_path = dir.join("s.sock");
        let stderr_path = dir.join("e.sock");
        let stdio_listener = UnixListener::bind(&stdio_path).unwrap();
        let stderr_listener = UnixListener::bind(&stderr_path).unwrap();
        // The guest's side: the stdio socket echoes, and the stderr socket speaks once.
        thread::spawn(move || {
            let (mut conn, _) = stdio_listener.accept().unwrap();
            let mut buf = [0u8; 64];
            let n = conn.read(&mut buf).unwrap();
            conn.write_all(&buf[..n]).unwrap();
        });
        thread::spawn(move || {
            let (mut conn, _) = stderr_listener.accept().unwrap();
            conn.write_all(b"warn\n").unwrap();
        });
        let (stdio, stderr) = (
            stdio_path.to_string_lossy().into_owned(),
            stderr_path.to_string_lossy().into_owned(),
        );
        let seen = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::new(Mutex::new(false));
        let gone = Arc::clone(&state);
        let port = fake_noded(
            move |method, path, _| match (method, path) {
                ("GET", "/v1/mcp-vm") => (200, r#"{"available":true,"reason":"ready"}"#.into()),
                ("POST", "/v1/mcp-runs") => (
                    201,
                    serde_json::json!({"id": "fetch-1", "stdio": stdio, "stderr": stderr})
                        .to_string(),
                ),
                ("GET", "/v1/mcp-runs/fetch-1") => {
                    let state = if *gone.lock().unwrap() {
                        "exited"
                    } else {
                        "running"
                    };
                    (
                        200,
                        serde_json::json!({"id": "fetch-1", "state": state}).to_string(),
                    )
                }
                ("DELETE", "/v1/mcp-runs/fetch-1") => {
                    *gone.lock().unwrap() = true;
                    (200, r#"{"removed":true}"#.into())
                }
                _ => (404, r#"{"error":"no such run"}"#.into()),
            },
            Arc::clone(&seen),
        );
        let launcher = VmLauncher::new(Upstream::new(port));
        assert_eq!(launcher.available(), Ok(()));
        let req = VmRequest {
            id: "fetch-1".into(),
            server: "fetch".into(),
            program: "/data/ro-1/node/bin/node".into(),
            args: vec![],
            env: BTreeMap::from([("HOME".to_string(), "/workspace/home".to_string())]),
            cwd: "/workspace".into(),
            storage: false,
            egress_port: 19128,
            mounts: vec![],
        };
        let mut child = launcher.launch(&req).unwrap();
        child.stdin().unwrap().write_all(b"ping\n").unwrap();
        let mut out = [0u8; 5];
        child.stdout().unwrap().read_exact(&mut out).unwrap();
        assert_eq!(&out, b"ping\n");
        let mut warn = String::new();
        child.stderr().unwrap().read_to_string(&mut warn).unwrap();
        assert!(warn.starts_with("warn"), "{warn}");
        assert!(!child.try_wait().unwrap());
        child.kill().unwrap();
        assert!(child.try_wait().unwrap());
        let posted = seen.lock().unwrap().clone();
        assert!(posted[0].starts_with("GET /v1/mcp-vm"), "{posted:?}");
        let body: Value = serde_json::from_str(posted[1].splitn(3, ' ').nth(2).unwrap()).unwrap();
        assert_eq!(body["program"], "/data/ro-1/node/bin/node");
        assert_eq!(body["env"]["HOME"], "/workspace/home");
        assert_eq!(body["egress_port"], 19128);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_node_that_cannot_boot_a_vm_says_why_and_refuses_a_run() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let port = fake_noded(
            |method, path, _| match (method, path) {
                ("GET", "/v1/mcp-vm") => (
                    200,
                    r#"{"available":false,"reason":"no /dev/kvm on this host"}"#.into(),
                ),
                _ => (503, r#"{"error":"the VM did not boot"}"#.into()),
            },
            Arc::clone(&seen),
        );
        let launcher = VmLauncher::new(Upstream::new(port));
        assert_eq!(
            launcher.available(),
            Err("no /dev/kvm on this host".to_string())
        );
        let req = VmRequest {
            id: "x-1".into(),
            server: "x".into(),
            program: "/data/ro-1/bin".into(),
            args: vec![],
            env: BTreeMap::new(),
            cwd: "/workspace".into(),
            storage: false,
            egress_port: 1,
            mounts: vec![],
        };
        let refused = launcher.launch(&req).err().unwrap();
        assert_eq!(refused, "the VM did not boot");
        let dead = Upstream::new(1);
        assert!(VmLauncher::new(dead).available().is_err());
    }
}
