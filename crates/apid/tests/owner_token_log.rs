//! The owner token never reaches apid's log. A scratch hub boots as a child process with a temp
//! state directory, config directory and ports, and every value it sees is fake. Its owner token
//! must land only in the 0600 `owner-token` file, and the log must name that file without the token.

use std::io::{BufRead, BufReader, Read};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The apid binary built for this test run.
const APID: &str = env!("CARGO_BIN_EXE_apid");
/// Ports the owner's own hub and daemons use. The test never picks one of them.
const OWNER_PORTS: [u16; 3] = [7700, 8899, 19080];
/// A fake value only. apid imports it into the scratch vault, and it must not reach the log either.
const FAKE_ENV_LINE: &str = "APZ_OWNER_TOKEN_LOG_TEST_KEY=fake-not-a-secret-0123";
/// apid's start, to its listening line: as generous as the shared harness's (a cold coverage run takes about thirty
/// seconds).
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);
const ATTEMPTS: u32 = 3;

/// A temp directory removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("apid-owner-log-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create the scratch directory");
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A child apid whose stdout and stderr are collected on reader threads. Killed on drop.
struct Hub {
    child: Child,
    stdout: Arc<Mutex<String>>,
    stderr: Arc<Mutex<String>>,
}

impl Hub {
    fn output(&self) -> (String, String) {
        let stdout = self
            .stdout
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let stderr = self
            .stderr
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        (stdout, stderr)
    }
}

impl Drop for Hub {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

enum Startup {
    Listening,
    Exited(ExitStatus),
    TimedOut,
}

/// Three loopback ports the OS just handed out, none of them the owner's.
fn free_ports() -> [u16; 3] {
    let mut held = Vec::new();
    let mut ports: Vec<u16> = Vec::new();
    while ports.len() < 3 {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .expect("bind a loopback port");
        let port = listener
            .local_addr()
            .expect("the listener's address")
            .port();
        if !OWNER_PORTS.contains(&port) && !ports.contains(&port) {
            ports.push(port);
        }
        held.push(listener);
    }
    drop(held);
    [ports[0], ports[1], ports[2]]
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

/// Starts apid with everything under `dir`. The error is the spawn failing, which is environmental.
fn boot(dir: &Path, ports: [u16; 3]) -> std::io::Result<Hub> {
    let state = dir.join("state");
    let config = dir.join("config");
    std::fs::create_dir_all(&config).expect("create the config directory");
    let env_file = dir.join("keys.env");
    std::fs::write(&env_file, format!("{FAKE_ENV_LINE}\n")).expect("write the fake env file");
    let mut command = apz_os::runner::command(Path::new(APID));
    command
        .arg("--port")
        .arg(ports[0].to_string())
        .arg("--hub-port")
        .arg(ports[1].to_string())
        .arg("--noded-port")
        .arg(ports[2].to_string())
        .arg("--state-dir")
        .arg(&state)
        .arg("--vault-key")
        .arg(config.join("vault.key"))
        .arg("--env-file")
        .arg(&env_file)
        .arg("--roles")
        .arg(dir.join("no-roles.json"))
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
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let stdout = Arc::new(Mutex::new(String::new()));
    let stderr = Arc::new(Mutex::new(String::new()));
    capture(
        child.stdout.take().expect("piped stdout"),
        Arc::clone(&stdout),
    );
    capture(
        child.stderr.take().expect("piped stderr"),
        Arc::clone(&stderr),
    );
    Ok(Hub {
        child,
        stdout,
        stderr,
    })
}

fn wait_for_listening(hub: &mut Hub) -> Startup {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if hub.output().0.contains("apid: listening on") {
            return Startup::Listening;
        }
        if let Ok(Some(status)) = hub.child.try_wait() {
            return Startup::Exited(status);
        }
        if Instant::now() >= deadline {
            return Startup::TimedOut;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// The log's lines without any that mention a token, for a failure message.
fn log_without_tokens(text: &str) -> String {
    text.lines()
        .filter(|line| !line.to_lowercase().contains("token"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn owner_token_is_never_logged() {
    let scratch = Scratch::new();
    let mut started: Option<(Hub, PathBuf)> = None;
    for attempt in 1..=ATTEMPTS {
        // A fresh directory per attempt: a retry must create its own owner token.
        let dir = scratch.0.join(format!("attempt-{attempt}"));
        let mut hub = match boot(&dir, free_ports()) {
            Ok(hub) => hub,
            Err(err) => {
                eprintln!(
                    "skipping: the apid binary cannot be started in this environment ({err})"
                );
                return;
            }
        };
        match wait_for_listening(&mut hub) {
            Startup::Listening => {
                started = Some((hub, dir));
                break;
            }
            Startup::Exited(status) => {
                let (_, stderr) = hub.output();
                // Another process took one of the picked ports between the probe and the bind: try again. Windows
                // words it "Only one usage of each socket address" (os error 10048).
                let taken = stderr.contains("in use") || stderr.contains("os error 10048");
                if taken && attempt < ATTEMPTS {
                    continue;
                }
                panic!(
                    "apid exited before it listened ({status:?}):\n{}",
                    log_without_tokens(&stderr)
                );
            }
            Startup::TimedOut => {
                panic!("apid did not print its listening line within {STARTUP_TIMEOUT:?}")
            }
        }
    }
    let (hub, dir) = started.expect("apid never started listening");
    let state = dir.join("state");

    let token_path = state.join("owner-token");
    let token = std::fs::read_to_string(&token_path)
        .expect("the owner-token file exists")
        .trim()
        .to_string();
    assert!(
        token.starts_with("apzo_") && token.len() > 16,
        "the owner-token file holds no owner token"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&token_path)
            .expect("stat the owner-token file")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "the owner-token file is not mode 0600");
    }

    let (stdout, stderr) = hub.output();
    assert!(
        !stdout.contains(&token),
        "the owner token was printed to stdout"
    );
    assert!(
        !stderr.contains(&token),
        "the owner token was written to stderr"
    );

    let created: Vec<&str> = stderr
        .lines()
        .filter(|line| line.contains("owner token created"))
        .collect();
    assert_eq!(
        created.len(),
        1,
        "expected exactly one `owner token created` line in the log"
    );
    assert!(
        created[0].contains(&token_path.display().to_string()),
        "the `owner token created` line does not name the owner-token file: {}",
        created[0]
    );
    assert!(
        created[0].contains("agentplaza hub login"),
        "the line does not name a command that uses the token"
    );

    // The hub's record is plaza.db's `hub_config` row: the digest of the token is there, the token never is.
    let mut record = Vec::new();
    for name in ["plaza.db", "plaza.db-wal"] {
        if let Ok(bytes) = std::fs::read(state.join(name)) {
            record.extend(bytes);
        }
    }
    assert!(!record.is_empty(), "plaza.db exists");
    assert!(
        !record.windows(token.len()).any(|w| w == token.as_bytes()),
        "plaza.db holds the owner token itself"
    );
}
