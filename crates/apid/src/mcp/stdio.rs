//! Stdio servers: a pinned package run under the OS sandbox, with the hub's pipes to its stdin and stdout.
//!
//! The sandbox (`apz_sandbox`) gives the child a deny-by-default filesystem and network: it reads its runtime and
//! its package, writes only its own directories, and reaches only the egress proxy's loopback port. Its environment
//! is the one the hub builds here (secret values included, as the server's own env vars, never argv or a file).
//! Each server has a directory `<state>/mcp/<id>/` (see [`Layout`]).

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use apz_sandbox::{Network, Sandboxed, Spec};
use serde_json::Value;

use super::client::{Error, Transport};
use super::egress::Lease;

/// The longest line a server may send (a larger one ends the connection).
const MAX_LINE: usize = 4 << 20;
/// The most stderr a failed install keeps for its message.
const TAIL_BYTES: usize = 4096;
/// How long an npm install may take.
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);
/// The text `apz_sandbox` support reports when the kernel cannot confine the network (no proxy guarantee).
const UNCONFINED: &str = "NETWORK IS NOT CONFINED";

/// Starts a sandboxed process with its three pipes. The stdio launcher uses the real sandbox; tests use the same.
pub trait Launcher: Send + Sync {
    fn launch(&self, spec: &Spec) -> Result<Sandboxed, String>;
}

/// A started server process, as the transport holds it: its three pipes, and its end. The sandbox's child
/// ([`Sandboxed`]) and a microVM's (`vm::VmChild`) both are one.
pub trait Child: Send {
    fn stdin(&mut self) -> Option<Box<dyn Write + Send>>;
    fn stdout(&mut self) -> Option<Box<dyn Read + Send>>;
    fn stderr(&mut self) -> Option<Box<dyn Read + Send>>;
    /// Whether the process has ended.
    fn try_wait(&mut self) -> std::io::Result<bool>;
    fn kill(&mut self) -> std::io::Result<()>;
    /// Waits until the process has ended.
    fn wait(&mut self) -> std::io::Result<()>;
}

impl Child for Sandboxed {
    fn stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        Sandboxed::stdin(self)
    }

    fn stdout(&mut self) -> Option<Box<dyn Read + Send>> {
        Sandboxed::stdout(self)
    }

    fn stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        Sandboxed::stderr(self)
    }

    fn try_wait(&mut self) -> std::io::Result<bool> {
        Ok(Sandboxed::try_wait(self)?.is_some())
    }

    fn kill(&mut self) -> std::io::Result<()> {
        Sandboxed::kill(self)
    }

    fn wait(&mut self) -> std::io::Result<()> {
        Sandboxed::wait(self).map(|_| ())
    }
}

/// The OS sandbox ([`apz_sandbox::spawn`]).
pub struct SandboxLauncher;

impl Launcher for SandboxLauncher {
    fn launch(&self, spec: &Spec) -> Result<Sandboxed, String> {
        if matches!(spec.network, Network::LoopbackPort(_)) {
            let support = apz_sandbox::support();
            if support.detail.contains(UNCONFINED) {
                return Err(format!(
                    "this system cannot confine a server's network ({}), so an MCP server that uses the network cannot run; remote (http) servers still work",
                    support.kind
                ));
            }
        }
        apz_sandbox::spawn(
            spec,
            std::process::Stdio::piped(),
            std::process::Stdio::piped(),
            std::process::Stdio::piped(),
        )
        .map_err(|e| format!("cannot start the server under the sandbox: {e}"))
    }
}

/// The directories a server owns, under `<state>/mcp/<id>/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub root: PathBuf,
    /// The npm prefix (`node_modules` lives here).
    pub pkg: PathBuf,
    /// Downloaded binaries, by SHA-256.
    pub bin: PathBuf,
    /// The server's working directory. Kept between runs with `storage.volume`, emptied at each start otherwise.
    pub data: PathBuf,
    pub home: PathBuf,
    pub tmp: PathBuf,
    /// npm's and uv's caches.
    pub cache: PathBuf,
    /// uv's tool environments and its managed Pythons.
    pub tools: PathBuf,
    pub python: PathBuf,
}

impl Layout {
    pub fn new(state_dir: &Path, server: &str) -> Layout {
        let root = state_dir.join("mcp").join(server);
        Layout {
            pkg: root.join("pkg"),
            bin: root.join("bin"),
            data: root.join("data"),
            home: root.join("home"),
            tmp: root.join("tmp"),
            cache: root.join("cache"),
            tools: root.join("tools"),
            python: root.join("python"),
            root,
        }
    }

    /// Creates the directories. `data` is emptied first unless the server keeps its storage.
    pub fn prepare(&self, keep_data: bool) -> Result<(), String> {
        if !keep_data {
            let _ = std::fs::remove_dir_all(&self.data);
        }
        for dir in [
            &self.pkg,
            &self.bin,
            &self.data,
            &self.home,
            &self.tmp,
            &self.cache,
            &self.tools,
            &self.python,
        ] {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
        }
        Ok(())
    }

    /// The directories a sandboxed process may write: its own, and nothing of another server's.
    pub fn writable(&self) -> Vec<PathBuf> {
        vec![
            self.data.clone(),
            self.home.clone(),
            self.tmp.clone(),
            self.cache.clone(),
            self.tools.clone(),
            self.python.clone(),
        ]
    }
}

/// The environment a sandboxed process gets: its own home and temp, its search path, and (when it reaches the
/// network) the proxy variables. Secret values are added by the caller.
pub fn base_env(
    layout: &Layout,
    path_dirs: &[PathBuf],
    proxy: Option<&str>,
) -> Vec<(String, String)> {
    let mut dirs = path_dirs.to_vec();
    if cfg!(windows) {
        dirs.push(PathBuf::from(
            std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()),
        ));
    } else {
        dirs.extend(["/usr/bin", "/bin", "/usr/sbin", "/sbin"].map(PathBuf::from));
    }
    let path = std::env::join_paths(dirs)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut env = vec![
        ("PATH".to_string(), path),
        (
            "HOME".to_string(),
            layout.home.to_string_lossy().into_owned(),
        ),
        (
            "TMPDIR".to_string(),
            layout.tmp.to_string_lossy().into_owned(),
        ),
        ("TMP".to_string(), layout.tmp.to_string_lossy().into_owned()),
        (
            "TEMP".to_string(),
            layout.tmp.to_string_lossy().into_owned(),
        ),
        (
            "XDG_CONFIG_HOME".to_string(),
            layout.home.join(".config").to_string_lossy().into_owned(),
        ),
        (
            "XDG_CACHE_HOME".to_string(),
            layout.cache.to_string_lossy().into_owned(),
        ),
        (
            "XDG_DATA_HOME".to_string(),
            layout
                .home
                .join(".local/share")
                .to_string_lossy()
                .into_owned(),
        ),
    ];
    if let Some(proxy) = proxy {
        env.extend(super::egress::proxy_env(proxy));
    }
    env
}

/// The arguments for npm's CLI, run under node: a pinned package installed into `prefix`. Lifecycle scripts are
/// not run (the package's own install code stays out of the process).
pub fn npm_install_args(npm_cli: &Path, prefix: &Path, name: &str, version: &str) -> Vec<String> {
    vec![
        npm_cli.to_string_lossy().into_owned(),
        "install".into(),
        "--prefix".into(),
        prefix.to_string_lossy().into_owned(),
        "--ignore-scripts".into(),
        "--no-audit".into(),
        "--no-fund".into(),
        "--no-update-notifier".into(),
        format!("{name}@{version}"),
    ]
}

/// The arguments for `uv tool run` of a pinned PyPI package: its command (`entry`, default the package name) with
/// the server's own arguments.
pub fn uv_run_args(name: &str, version: &str, entry: Option<&str>, args: &[String]) -> Vec<String> {
    let mut out = vec![
        "tool".to_string(),
        "run".to_string(),
        "--from".to_string(),
        format!("{name}=={version}"),
        entry.unwrap_or(name).to_string(),
    ];
    out.extend(args.iter().cloned());
    out
}

/// The script an installed npm package runs: the `bin` its `package.json` names. A package with several bins
/// must name the one that matches the package, or be given one.
pub fn npm_bin(package_dir: &Path, name: &str) -> Result<PathBuf, String> {
    let text = std::fs::read_to_string(package_dir.join("package.json"))
        .map_err(|e| format!("the package {name} has no package.json: {e}"))?;
    let json: Value = serde_json::from_str(&text).map_err(|e| format!("bad package.json: {e}"))?;
    let bin = match json.get("bin") {
        Some(Value::String(path)) => path.clone(),
        Some(Value::Object(map)) => {
            let plain = name.rsplit('/').next().unwrap_or(name);
            let picked = match map.len() {
                1 => map.values().next(),
                _ => map.get(plain),
            };
            picked
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("the package {name} names no single bin"))?
        }
        _ => return Err(format!("the package {name} names no bin")),
    };
    let path = Path::new(&bin);
    if path.is_absolute() || path.components().any(|c| c.as_os_str() == "..") {
        return Err(format!("the package {name} names a bin outside itself"));
    }
    Ok(package_dir.join(path))
}

/// Runs a sandboxed process to its end. Its stdout is drained and its stderr's last lines kept, so a failure can
/// say what it printed. It is killed at `timeout`.
pub fn run_to_end(launcher: &dyn Launcher, spec: &Spec, timeout: Duration) -> Result<(), String> {
    let mut child = launcher.launch(spec)?;
    drop(child.stdin());
    if let Some(mut out) = child.stdout() {
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut out, &mut std::io::sink());
        });
    }
    let tail = child.stderr().map(|mut err| {
        std::thread::spawn(move || {
            let mut kept: VecDeque<u8> = VecDeque::new();
            let mut buf = [0u8; 4096];
            while let Ok(n) = err.read(&mut buf) {
                if n == 0 {
                    break;
                }
                for byte in &buf[..n] {
                    if kept.len() == TAIL_BYTES {
                        kept.pop_front();
                    }
                    kept.push_back(*byte);
                }
            }
            String::from_utf8_lossy(&kept.into_iter().collect::<Vec<u8>>()).into_owned()
        })
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let text = tail.and_then(|t| t.join().ok()).unwrap_or_default();
                if status.success() {
                    return Ok(());
                }
                let last = text
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .trim();
                return Err(if last.is_empty() {
                    format!("the install exited with {status}")
                } else {
                    format!("the install exited with {status}: {last}")
                });
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "the install did not finish in {} s",
                    timeout.as_secs()
                ));
            }
            Err(e) => return Err(format!("cannot wait for the install: {e}")),
        }
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
    child: Box<dyn Child>,
    stdin: Option<Box<dyn Write + Send>>,
    lines: Receiver<Result<String, ()>>,
    closed: bool,
    /// The proxy credential the process holds, freed when the transport drops.
    _lease: Option<Lease>,
}

impl StdioTransport {
    /// Takes over a started process. `on_stderr` gets every stderr line, as the child wrote it.
    pub fn new(
        mut child: Box<dyn Child>,
        on_stderr: Arc<dyn Fn(String) + Send + Sync>,
        lease: Option<Lease>,
    ) -> Result<StdioTransport, String> {
        let stdin = child.stdin().ok_or("no stdin")?;
        let stdout = child.stdout().ok_or("no stdout")?;
        let stderr = child.stderr().ok_or("no stderr")?;
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
        !self.closed && matches!(self.child.try_wait(), Ok(false))
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
    fn a_server_keeps_its_directories_under_its_own_root() {
        let layout = Layout::new(Path::new("/state"), "github");
        assert_eq!(layout.root, PathBuf::from("/state/mcp/github"));
        assert_eq!(layout.data, PathBuf::from("/state/mcp/github/data"));
        assert!(
            layout
                .writable()
                .iter()
                .all(|p| p.starts_with(&layout.root))
        );
        assert!(!layout.writable().contains(&layout.pkg));
    }

    #[test]
    fn npm_installs_a_pinned_package_without_scripts_and_names_its_bin() {
        let args = npm_install_args(
            Path::new("/rt/node/npm-cli.js"),
            Path::new("/state/mcp/memory/pkg"),
            "@modelcontextprotocol/server-memory",
            "2.0.0",
        );
        assert_eq!(args[0], "/rt/node/npm-cli.js");
        assert!(args.contains(&"--ignore-scripts".to_string()));
        assert_eq!(
            args.last().unwrap(),
            "@modelcontextprotocol/server-memory@2.0.0"
        );
        let dir = std::env::temp_dir().join(format!("apz-npm-bin-{}", std::process::id()));
        let pkg = dir.join("node_modules/@modelcontextprotocol/server-memory");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(
            pkg.join("package.json"),
            r#"{"name":"x","bin":{"mcp-server-memory":"dist/index.js"}}"#,
        )
        .unwrap();
        let bin = npm_bin(&pkg, "@modelcontextprotocol/server-memory").unwrap();
        assert_eq!(bin, pkg.join("dist/index.js"));
        std::fs::write(pkg.join("package.json"), r#"{"bin":"../escape.js"}"#).unwrap();
        assert!(npm_bin(&pkg, "x").unwrap_err().contains("outside itself"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn uv_runs_the_pinned_version_of_its_entry() {
        assert_eq!(
            uv_run_args("mcp-server-time", "2026.10.19", None, &[]),
            [
                "tool",
                "run",
                "--from",
                "mcp-server-time==2026.10.19",
                "mcp-server-time"
            ]
        );
        assert_eq!(
            uv_run_args(
                "mcp-server-time",
                "2026.10.19",
                Some("other"),
                &["--local".into()]
            ),
            [
                "tool",
                "run",
                "--from",
                "mcp-server-time==2026.10.19",
                "other",
                "--local"
            ]
        );
    }

    #[test]
    fn the_base_env_points_home_and_temp_into_the_server_and_proxies_the_network() {
        let layout = Layout::new(Path::new("/state"), "fetch");
        let env = base_env(
            &layout,
            &[PathBuf::from("/rt/node/bin")],
            Some("http://apz:t@127.0.0.1:19128"),
        );
        let get = |k: &str| {
            env.iter()
                .find(|(name, _)| name == k)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(get("HOME").unwrap(), "/state/mcp/fetch/home");
        assert!(get("PATH").unwrap().starts_with("/rt/node/bin"));
        assert_eq!(get("HTTPS_PROXY").unwrap(), "http://apz:t@127.0.0.1:19128");
        let offline = base_env(&layout, &[], None);
        assert!(!offline.iter().any(|(k, _)| k == "HTTPS_PROXY"));
    }
}
