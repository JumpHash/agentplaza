//! This machine as a node of another hub, driven from the owner's console: join the hub a join code
//! names (`apz-link join`), see the link, leave it. Once joined, `apz-link run` keeps the node
//! connected; apid starts it detached, records its pid in `state/apz-link.pid` and appends its output
//! to `state/logs/apz-link.log`. The identity (`node.json`, `node.key`) sits in the node directory
//! `apz_os::dirs::data()/node`, where `apz-link` itself keeps it.
//!
//! Owner routes (owner auth applies):
//!   GET  /v1/node-link                  → {joined, hub, node_id, cids, running, pid}
//!   POST /v1/node-link/join   {code}    → the status (409 when already joined)
//!   POST /v1/node-link/leave  {forget?} → stops the link; with forget, deletes the identity → the status

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use apz_http::{Response, Router};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::state::App;

/// The longest join code accepted, in characters.
const MAX_CODE: usize = 4096;
/// How long `apz-link join` may run before it is killed.
const JOIN_LIMIT: Duration = Duration::from_secs(60);
/// How long `leave` waits for the link to exit after asking it to stop.
const STOP_WAIT: Duration = Duration::from_secs(5);

/// Set while a join runs, so a second join is refused instead of racing the first.
static JOINING: AtomicBool = AtomicBool::new(false);

/// The link as the identity files and the pid file say it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub joined: bool,
    pub hub: Option<String>,
    pub node_id: Option<String>,
    /// The inclusive guest-CID range, `A-B`.
    pub cids: Option<String>,
    pub running: bool,
    pub pid: Option<u32>,
}

impl Status {
    fn json(&self) -> Value {
        json!({
            "joined": self.joined,
            "hub": self.hub,
            "node_id": self.node_id,
            "cids": self.cids,
            "running": self.running,
            "pid": self.pid,
        })
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct JoinBody {
    code: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LeaveBody {
    forget: bool,
}

/// The node directory `apz-link` keeps its identity in.
fn identity_dir() -> PathBuf {
    apz_os::dirs::data().join("node")
}

/// The pid file of the `apz-link run` this app started.
fn pid_file() -> PathBuf {
    apz_os::dirs::state().join("apz-link.pid")
}

/// The `apz-link` binary beside apid, else the one on `PATH`.
fn link_binary() -> PathBuf {
    let name = apz_os::process::exe("apz-link");
    std::env::current_exe()
        .ok()
        .map(|exe| exe.with_file_name(&name))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Whether `code` can be a join code: non-empty, at most [`MAX_CODE`] characters, no whitespace or
/// control characters, and not one of the option words `apz-link` scans for before its command.
pub fn valid_code(code: &str) -> Result<(), String> {
    if code.is_empty() {
        return Err("the join code is empty".into());
    }
    if code.chars().count() > MAX_CODE {
        return Err(format!(
            "the join code is longer than {MAX_CODE} characters"
        ));
    }
    if code.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("the join code contains whitespace or control characters".into());
    }
    if matches!(code, "--state" | "--version" | "-V") {
        return Err("that is not a join code".into());
    }
    Ok(())
}

/// The pid in a pid file's text: a positive decimal number.
pub fn parse_pid(text: &str) -> Option<u32> {
    text.trim().parse::<u32>().ok().filter(|pid| *pid > 0)
}

/// Whether process `pid` exists: `/proc` on Linux, `kill -0` on other Unix, `tasklist` on Windows.
fn alive(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        Path::new("/proc").join(pid.to_string()).exists()
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .stdin(Stdio::null())
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).contains(&format!(" {pid} ")))
            .unwrap_or(false)
    }
}

/// The link as the node directory and the pid file say it is.
fn status_in(identity_dir: &Path, pid_path: &Path) -> Status {
    let node: Option<Value> = fs::read(identity_dir.join("node.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let field = |key: &str| node.as_ref().and_then(|n| n.get(key));
    let hub = field("hub_url").and_then(Value::as_str).map(str::to_string);
    let node_id = field("node_id").and_then(Value::as_str).map(str::to_string);
    let base = field("cid_base").and_then(Value::as_u64);
    let count = field("cid_count").and_then(Value::as_u64);
    let joined = hub.is_some() && node_id.is_some() && base.is_some() && count.is_some();
    let cids = match (base, count) {
        (Some(base), Some(count)) if count > 0 => Some(format!("{}-{}", base, base + count - 1)),
        _ => None,
    };
    let pid = fs::read_to_string(pid_path)
        .ok()
        .and_then(|text| parse_pid(&text))
        .filter(|pid| alive(*pid));
    Status {
        joined,
        hub,
        node_id,
        cids,
        running: pid.is_some(),
        pid,
    }
}

/// The link as this machine has it now.
pub fn status() -> Status {
    status_in(&identity_dir(), &pid_file())
}

/// Parses a request body as `T`; an empty body is the default.
fn body_as<T: DeserializeOwned + Default>(body: &[u8]) -> Result<T, String> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|e| format!("bad JSON: {e}"))
}

fn error(status: u16, message: &str) -> Response {
    Response::json(status, &json!({ "error": message }))
}

/// `POST /v1/node-link/join {code}`: joins the hub the code names and starts the link.
fn join_route(body: &[u8]) -> Response {
    let code = match body_as::<JoinBody>(body) {
        Ok(join) => join.code,
        Err(e) => return error(400, &e),
    };
    if let Err(e) = valid_code(&code) {
        return error(400, &e);
    }
    if status().joined {
        return error(409, "this machine is already a node; leave the hub first");
    }
    if JOINING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return error(409, "a join is already running");
    }
    let result = join_and_run(&code);
    JOINING.store(false, Ordering::SeqCst);
    match result {
        Ok(()) => Response::json(200, &status().json()),
        Err(e) => error(400, &e),
    }
}

/// Reads a pipe to its end on a thread of its own, so the child never blocks on a full pipe.
fn read_all<R: Read + Send + 'static>(mut pipe: R) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        buf
    })
}

/// Runs `apz-link join <code>` (at most [`JOIN_LIMIT`]), then starts `apz-link run` unless it runs.
/// The error text never contains the code.
fn join_and_run(code: &str) -> Result<(), String> {
    let mut child = Command::new(link_binary())
        .arg("join")
        .arg(code)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("apz-link: {e}"))?;
    let stderr = child.stderr.take().map(read_all);
    let deadline = Instant::now() + JOIN_LIMIT;
    let succeeded = loop {
        match child.try_wait() {
            Ok(Some(exit)) => break exit.success(),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("apz-link join did not finish within 60 seconds".into());
            }
            Ok(None) => thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(format!("apz-link join: {e}")),
        }
    };
    if !succeeded {
        let err = stderr
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default();
        let text = String::from_utf8_lossy(&err).trim().replace(code, "<code>");
        return Err(if text.is_empty() {
            "apz-link join failed".into()
        } else {
            text
        });
    }
    let now = status();
    if !now.joined {
        return Err("apz-link join wrote no identity".into());
    }
    if now.running {
        return Ok(());
    }
    start_run()
}

/// Starts `apz-link run` detached, its output appended to the link's log, and records its pid.
fn start_run() -> Result<(), String> {
    let logs = apz_os::dirs::state().join("logs");
    apz_os::private::create_dir(&logs).map_err(|e| format!("{}: {e}", logs.display()))?;
    let log_path = logs.join("apz-link.log");
    let log =
        apz_os::private::append(&log_path).map_err(|e| format!("{}: {e}", log_path.display()))?;
    let errors = log.try_clone().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(link_binary());
    cmd.arg("run")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(errors));
    apz_os::process::detach(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| format!("apz-link run: {e}"))?;
    let pid = child.id();
    // Reaped in the background: an exited link must not linger as a zombie that looks alive.
    thread::spawn(move || {
        let _ = child.wait();
    });
    let path = pid_file();
    apz_os::private::write(&path, pid.to_string().as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Asks process `pid` to stop: `kill` (SIGTERM) on Unix, `taskkill` on Windows.
fn signal(pid: u32) {
    let mut cmd = if cfg!(windows) {
        let mut cmd = Command::new("taskkill");
        cmd.args(["/PID", &pid.to_string(), "/F"]);
        cmd
    } else {
        let mut cmd = Command::new("kill");
        cmd.arg(pid.to_string());
        cmd
    };
    let _ = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Stops the recorded `apz-link run` (waiting at most [`STOP_WAIT`] for it to exit) and forgets its pid.
fn stop_run() -> Result<(), String> {
    let path = pid_file();
    let pid = fs::read_to_string(&path)
        .ok()
        .and_then(|text| parse_pid(&text));
    if let Some(pid) = pid.filter(|pid| alive(*pid)) {
        signal(pid);
        let deadline = Instant::now() + STOP_WAIT;
        while alive(pid) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(100));
        }
        if alive(pid) {
            return Err(format!("apz-link (pid {pid}) did not stop"));
        }
    }
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// `POST /v1/node-link/leave {forget?}`: stops `apz-link run`; with `forget`, deletes the identity.
fn leave_route(body: &[u8]) -> Response {
    let forget = match body_as::<LeaveBody>(body) {
        Ok(leave) => leave.forget,
        Err(e) => return error(400, &e),
    };
    if let Err(e) = stop_run() {
        return error(500, &e);
    }
    if forget {
        let dir = identity_dir();
        if let Err(e) = fs::remove_dir_all(&dir) {
            if e.kind() != std::io::ErrorKind::NotFound {
                return error(500, &format!("{}: {e}", dir.display()));
            }
        }
    }
    Response::json(200, &status().json())
}

/// `GET /v1/node-link`, `POST /v1/node-link/join` and `POST /v1/node-link/leave`.
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/node-link", |_, _, _| {
            Response::json(200, &status().json())
        })
        .post("/v1/node-link/join", |_, req, _| join_route(&req.body))
        .post("/v1/node-link/leave", |_, req, _| leave_route(&req.body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("apid-node-link-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn join_codes_are_checked_for_shape() {
        assert!(valid_code("abc-DEF_123").is_ok());
        assert!(valid_code("").is_err());
        assert!(valid_code("ab cd").is_err());
        assert!(valid_code("ab\tcd").is_err());
        assert!(valid_code("ab\ncd").is_err());
        assert!(valid_code("--state").is_err());
        assert!(valid_code(&"x".repeat(MAX_CODE)).is_ok());
        assert!(valid_code(&"x".repeat(MAX_CODE + 1)).is_err());
    }

    #[test]
    fn status_reads_the_identity_apz_link_writes_and_rejects_malformed_ones() {
        // Status reads the identity apz-link writes.
        {
            let root = scratch("status");
            let identity = root.join("node");
            let pid_path = root.join("apz-link.pid");

            let none = status_in(&identity, &pid_path);
            assert!(!none.joined && none.hub.is_none() && none.cids.is_none() && !none.running);

            // The files apz-link save writes: node.json (0600) beside node.key (the 32-byte seed).
            fs::create_dir_all(&identity).unwrap();
            let stored = json!({
                "hub_url": "https://hub.example:8443",
                "pin": "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHB",
                "node_id": "node-7",
                "cid_base": 3,
                "cid_count": 2,
                "hub_name": "hub",
            });
            fs::write(
                identity.join("node.json"),
                serde_json::to_vec_pretty(&stored).unwrap(),
            )
            .unwrap();
            fs::write(identity.join("node.key"), [9u8; 32]).unwrap();
            let joined = status_in(&identity, &pid_path);
            assert!(joined.joined);
            assert_eq!(joined.hub.as_deref(), Some("https://hub.example:8443"));
            assert_eq!(joined.node_id.as_deref(), Some("node-7"));
            assert_eq!(joined.cids.as_deref(), Some("3-4"));
            assert!(!joined.running && joined.pid.is_none());

            // A pid file naming a live process (this one) is running; a dead pid is not.
            fs::write(&pid_path, format!("{}\n", std::process::id())).unwrap();
            let live = status_in(&identity, &pid_path);
            assert!(live.running);
            assert_eq!(live.pid, Some(std::process::id()));
            fs::write(&pid_path, "4000000000").unwrap();
            assert!(!status_in(&identity, &pid_path).running);

            let _ = fs::remove_dir_all(&root);
        }
        // A malformed identity is not joined.
        {
            let root = scratch("malformed");
            fs::write(
                root.join("node.json"),
                b"{\"hub_url\": \"https://hub.example\"}",
            )
            .unwrap();
            let status = status_in(&root, &root.join("missing.pid"));
            assert!(!status.joined);
            assert!(status.hub.as_deref() == Some("https://hub.example"));
            assert!(status.node_id.is_none() && status.cids.is_none());
            let _ = fs::remove_dir_all(&root);
        }
    }
}
