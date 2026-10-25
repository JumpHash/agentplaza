//! A base build's VM over the test transport (PLAN 16.4): the harness process dials a Unix socket named as its guest's
//! vsock port would be (`vm<cid>_<port>`), says hello as a session of kind `base` with no model, and answers the hub's
//! exec items. The hub's side is scripted here, as vsockd is in production.

#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The guest CID the socket is named for.
const CID: u32 = 500;
const WAIT: Duration = Duration::from_secs(20);

/// Accepts the harness's connection on `listener`, for at most [`WAIT`].
fn accept(listener: &UnixListener) -> UnixStream {
    listener
        .set_nonblocking(true)
        .expect("a nonblocking listener");
    let deadline = Instant::now() + WAIT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).expect("a blocking stream");
                stream.set_read_timeout(Some(WAIT)).expect("a read timeout");
                return stream;
            }
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            Err(err) => panic!("the harness never dialled: {err}"),
        }
    }
}

/// The next line of the harness's that is of `kind`, skipping its heartbeats and stats.
fn next_of(lines: &mut impl Iterator<Item = std::io::Result<String>>, kind: &str) -> Value {
    loop {
        let line = lines
            .next()
            .expect("the harness closed the connection")
            .expect("a line from the harness");
        let value: Value = serde_json::from_str(&line).expect("a JSON line");
        if value["type"] == kind {
            return value;
        }
    }
}

fn send(stream: &mut UnixStream, message: &Value) {
    let mut line = message.to_string();
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .expect("a line to the harness");
}

#[test]
fn a_base_session_says_hello_as_its_kind_and_answers_the_exec_items() {
    let dir = std::env::temp_dir().join(format!("apz-base-exec-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch folder");
    let socket = dir.join(format!("vm{CID}_5000"));
    let listener = UnixListener::bind(&socket).expect("the test socket");

    let mut harness: Child = Command::new(env!("CARGO_BIN_EXE_agent-harness"))
        .arg("--cmdline")
        .arg(format!(
            "plaza.harness=1 plaza.keepalive=1 plaza.kind=base plaza.vm=0 plaza.connect=uds:{}",
            socket.display()
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the harness starts");

    let mut stream = accept(&listener);
    let mut lines = BufReader::new(stream.try_clone().expect("a second handle")).lines();
    let hello = next_of(&mut lines, "hello");
    assert_eq!(hello["kind"], "base", "{hello}");
    assert_eq!(hello["keepalive"], true, "{hello}");
    assert_eq!(hello["guest_cid"], CID, "{hello}");
    assert!(hello.get("role").is_none(), "a base has no role: {hello}");

    send(
        &mut stream,
        &json!({"type": "welcome", "echo": "plaza-orchestrator", "keepalive": true}),
    );

    // An item outside the workspace is refused, and its answer says why; the session stays up for the next one.
    send(
        &mut stream,
        &json!({"type": "exec", "id": 1, "argv": ["true"], "cwd": "/tmp", "timeout_s": 30}),
    );
    let refused = next_of(&mut lines, "exec_result");
    assert_eq!(refused["id"], 1, "{refused}");
    assert_eq!(refused["ok"], false, "{refused}");
    assert!(refused["exit_code"].is_null(), "{refused}");
    assert!(
        refused["output"]
            .as_str()
            .unwrap_or_default()
            .contains("is not inside"),
        "{refused}"
    );

    // An item with no command is refused too, with no process started.
    send(
        &mut stream,
        &json!({"type": "exec", "id": 2, "argv": [], "cwd": "/workspace", "timeout_s": 30}),
    );
    let empty = next_of(&mut lines, "exec_result");
    assert_eq!(empty["id"], 2, "{empty}");
    assert_eq!(empty["ok"], false, "{empty}");

    // The hub stops the base's session: the harness answers with its bye, and ends cleanly.
    send(
        &mut stream,
        &json!({"type": "stop", "reason": "build done"}),
    );
    let bye = next_of(&mut lines, "bye");
    assert_eq!(bye["calls"], 0, "{bye}");
    let status = harness.wait().expect("the harness ends");
    assert!(status.success(), "{status}");
    let _ = std::fs::remove_dir_all(&dir);
}
