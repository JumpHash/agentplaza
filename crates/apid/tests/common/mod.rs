//! A scratch hub for the integration tests: apid as a child process with everything in a temp directory, an empty
//! env file and ports none of the owner's daemons use. The agents' VMs are played by calls to the internal API, as
//! the agent gateway makes them, and the daemons apid calls (the gateway, noded) can be stood in for
//! ([`FakeDaemon`]).

// Each test crate uses its own part of this module.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// The apid binary built for this test run.
/// How much slower than a developer machine this run is: `APZ_PERF_SLACK` when set and positive (the arm64
/// tests under qemu set it), else 1. Waits scale by it, so an emulated run waits longer before it fails.
#[allow(dead_code)] // not every test binary that includes `common` waits
pub fn slack() -> f64 {
    std::env::var("APZ_PERF_SLACK")
        .ok()
        .and_then(|text| text.parse::<f64>().ok())
        .filter(|slack| *slack > 0.0)
        .unwrap_or(1.0)
}

const APID: &str = env!("CARGO_BIN_EXE_apid");
/// Ports the owner's own hub and daemons use. A test never picks one of them.
const OWNER_PORTS: [u16; 3] = [7700, 8899, 19080];
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
/// One request's answer. Generous: a busy CI runner (thousands of tests at once) can take over ten seconds for a
/// call that compiles a Weft language pack on its first use (review_tree), and a hang still fails.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);
const ATTEMPTS: u32 = 3;

/// A temp directory removed on drop.
pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Scratch {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("apid-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create the scratch directory");
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A child apid, its output collected on reader threads. Killed on drop.
pub struct Hub {
    child: Child,
    output: Arc<Mutex<String>>,
    pub port: u16,
    /// The internal token the agent gateway sends; apid writes it to its state dir.
    token: String,
    /// The directory of the attempt that came up: its state and config.
    dir: PathBuf,
}

impl Drop for Hub {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Hub {
    /// Stops the hub as a stop signal does (its last state writes run), waits for it to exit, and starts it again on
    /// the same state: the directory of the attempt that came up, not a fresh one.
    #[allow(dead_code)] // not every test binary that includes `common` restarts its hub
    pub fn restart_with_env(
        mut self,
        gateway: Option<u16>,
        noded: Option<u16>,
        roles: Option<&Value>,
        env: &[(&str, &str)],
    ) -> Hub {
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status();
        let _ = self.child.wait();
        let dir = self.dir.clone();
        drop(self);
        (1..=ATTEMPTS)
            .find_map(|_| boot(&dir, gateway, noded, roles, env))
            .expect("the hub comes up again")
    }
}

impl Hub {
    /// A scratch hub in `scratch`, whose gateway and noded are at `gateway` and `noded` when given (else ports nothing
    /// listens on).
    pub fn start(scratch: &Scratch, gateway: Option<u16>, noded: Option<u16>) -> Hub {
        Hub::start_with_roles(scratch, gateway, noded, None)
    }

    /// [`Hub::start`] with a roles manifest (`roles`, the JSON of one) instead of none.
    pub fn start_with_roles(
        scratch: &Scratch,
        gateway: Option<u16>,
        noded: Option<u16>,
        roles: Option<&Value>,
    ) -> Hub {
        Hub::start_with_env(scratch, gateway, noded, roles, &[])
    }

    /// [`Hub::start_with_roles`] with environment variables of its own (`WEFT_BIN` to a missing file: no Weft).
    pub fn start_with_env(
        scratch: &Scratch,
        gateway: Option<u16>,
        noded: Option<u16>,
        roles: Option<&Value>,
        env: &[(&str, &str)],
    ) -> Hub {
        (1..=ATTEMPTS)
            .find_map(|attempt| {
                boot(
                    &scratch.0.join(format!("attempt-{attempt}")),
                    gateway,
                    noded,
                    roles,
                    env,
                )
            })
            .expect("a scratch hub comes up")
    }

    /// The internal token the agent gateway presents (a VM's calls carry it).
    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn log(&self) -> String {
        self.output
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// The owner's call (a loopback request is the owner's on this hub).
    pub fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        self.send(method, path, &[], body)
    }

    /// A call from agent `cid`'s VM, as the agent gateway relays it.
    pub fn guest(&self, cid: u64, what: &str, body: Value) -> (u16, Value) {
        let path = format!("/internal/v1/agents/{cid}/{what}");
        self.send(
            "POST",
            &path,
            &[("x-apz-internal", &self.token)],
            Some(body),
        )
    }

    /// A call from the agent gateway itself (vsockd's internal routes), with the machine's internal token.
    #[allow(dead_code)] // not every test binary that includes `common` uses it
    pub fn internal(&self, path: &str, body: Value) -> (u16, Value) {
        self.send("POST", path, &[("x-apz-internal", &self.token)], Some(body))
    }

    /// Agent `cid`'s VM says the work item `work` ended as `status` (`done`, `failed`, `stopped`), with `summary`.
    pub fn work_done(&self, cid: u64, work: u64, status: &str, summary: &str) {
        let message =
            json!({ "type": "work_done", "work": work, "status": status, "summary": summary });
        let (code, body) = self.guest(cid, "message", message);
        assert_eq!(code, 200, "{body}");
    }

    /// Waits up to `timeout` (times [`slack`]) for `ready` to hold of the owner's `GET path`; the last answer
    /// either way.
    pub fn wait_for(&self, path: &str, timeout: Duration, ready: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + timeout.mul_f64(slack());
        loop {
            let (_, body) = self.call("GET", path, None);
            if ready(&body) || Instant::now() >= deadline {
                return body;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    /// A raw call with headers of its own (a connector's signed delivery): the reply's status and its JSON body.
    #[allow(dead_code)] // not every test binary that includes `common` uses it
    pub fn send_with_headers(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        bytes: &[u8],
    ) -> (u16, Value) {
        let reply = apz_http::client::send_with(
            self.port,
            method,
            path,
            headers,
            Some(bytes),
            CALL_TIMEOUT,
        )
        .unwrap_or_else(|err| panic!("{method} {path}: {err}\n{}", self.log()));
        let value = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
        (reply.status, value)
    }

    /// The owner's call with a raw body (a file's bytes) of `content_type`.
    pub fn send_bytes(
        &self,
        method: &str,
        path: &str,
        bytes: &[u8],
        content_type: &str,
    ) -> (u16, Value) {
        let reply = apz_http::client::send_with(
            self.port,
            method,
            path,
            &[("content-type", content_type)],
            Some(bytes),
            CALL_TIMEOUT,
        )
        .unwrap_or_else(|err| panic!("{method} {path}: {err}\n{}", self.log()));
        let value = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
        (reply.status, value)
    }

    fn send(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<Value>,
    ) -> (u16, Value) {
        let body = body.map(|body| body.to_string());
        let reply = apz_http::client::send_with(
            self.port,
            method,
            path,
            headers,
            body.as_deref().map(str::as_bytes),
            CALL_TIMEOUT,
        )
        .unwrap_or_else(|err| panic!("{method} {path}: {err}\n{}", self.log()));
        let value = serde_json::from_slice(&reply.body).unwrap_or(Value::Null);
        (reply.status, value)
    }
}

/// A loopback port the OS just handed out, none of the owner's.
fn free_port() -> u16 {
    loop {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .expect("bind a loopback port");
        let port = listener
            .local_addr()
            .expect("the listener's address")
            .port();
        if !OWNER_PORTS.contains(&port) {
            return port;
        }
    }
}

fn capture<R: Read + Send + 'static>(reader: R, sink: Arc<Mutex<String>>) {
    thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            let mut text = sink.lock().unwrap_or_else(|p| p.into_inner());
            text.push_str(&line);
            text.push('\n');
        }
    });
}

/// Starts apid with everything under `dir` and waits until it listens. `None` when it does not come up (a port
/// taken in between, say); the caller tries again.
fn boot(
    dir: &Path,
    gateway: Option<u16>,
    noded: Option<u16>,
    roles: Option<&Value>,
    env: &[(&str, &str)],
) -> Option<Hub> {
    let port = free_port();
    let state = dir.join("state");
    let config = dir.join("config");
    std::fs::create_dir_all(&config).expect("create the config directory");
    let env_file = dir.join("empty.env");
    std::fs::write(&env_file, "").expect("write the empty env file");
    let manifest = dir.join("roles.json");
    if let Some(roles) = roles {
        std::fs::write(&manifest, roles.to_string()).expect("write the roles manifest");
    }
    let mut command = apz_os::runner::command(Path::new(APID));
    command
        .arg("--port")
        .arg(port.to_string())
        .arg("--hub-port")
        .arg(gateway.unwrap_or_else(free_port).to_string())
        .arg("--noded-port")
        .arg(noded.unwrap_or_else(free_port).to_string())
        .arg("--state-dir")
        .arg(&state)
        .arg("--vault-key")
        .arg(config.join("vault.key"))
        .arg("--env-file")
        .arg(&env_file)
        .arg("--roles")
        .arg(&manifest)
        .arg("--prompts")
        .arg(dir.join("no-prompts"))
        .arg("--console")
        .arg(dir.join("no-console"))
        // Nothing the owner has on this machine: every location apid reads is inside the scratch dir.
        .env("HOME", dir)
        .env("XDG_STATE_HOME", dir.join("xdg-state"))
        .env("XDG_CONFIG_HOME", dir.join("xdg-config"))
        .env("XDG_DATA_HOME", dir.join("xdg-data"))
        .env("APZ_SHARE", dir.join("no-share"))
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().ok()?;
    let output = Arc::new(Mutex::new(String::new()));
    capture(
        child.stdout.take().expect("piped stdout"),
        Arc::clone(&output),
    );
    capture(
        child.stderr.take().expect("piped stderr"),
        Arc::clone(&output),
    );
    let deadline = Instant::now() + STARTUP_TIMEOUT.mul_f64(slack());
    loop {
        let listening = output
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains("apid: listening on");
        if listening {
            break;
        }
        if matches!(child.try_wait(), Ok(Some(_))) || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let token = std::fs::read_to_string(state.join("internal.token"))
        .expect("apid writes its internal token")
        .trim()
        .to_string();
    Some(Hub {
        child,
        output,
        port,
        token,
        dir: dir.to_path_buf(),
    })
}

/// One request a [`FakeDaemon`] was sent: method, path (with its query) and body.
pub type Seen = (String, String, String);

/// A stand-in for a daemon apid calls (the agent gateway, noded): it answers each request with
/// `answer(method, path, body)` and keeps what it was sent.
pub struct FakeDaemon {
    pub port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl FakeDaemon {
    pub fn serve(
        answer: impl Fn(&str, &str, &str) -> (u16, String) + Send + Sync + 'static,
    ) -> FakeDaemon {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .expect("bind the fake daemon");
        let port = listener.local_addr().expect("its address").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let answer = Arc::new(answer);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (log, answer) = (Arc::clone(&log), Arc::clone(&answer));
                thread::spawn(move || {
                    let _ = serve_one(stream, &log, &*answer);
                });
            }
        });
        FakeDaemon { port, seen }
    }

    /// The requests it was sent so far, oldest first.
    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

/// Answers one request on `stream` (HTTP/1.1, a Content-Length body, then the connection closes).
fn serve_one(
    mut stream: TcpStream,
    log: &Mutex<Vec<Seen>>,
    answer: &(dyn Fn(&str, &str, &str) -> (u16, String) + Send + Sync),
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            break;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let body = String::from_utf8_lossy(&body).into_owned();
    let (status, reply) = answer(&method, &path, &body);
    log.lock()
        .unwrap_or_else(|p| p.into_inner())
        .push((method, path, body));
    write!(
        stream,
        "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len()
    )?;
    stream.flush()
}

/// The text of a JSON string value ("" for anything else).
pub fn text(value: &Value) -> String {
    value.as_str().unwrap_or_default().to_string()
}
