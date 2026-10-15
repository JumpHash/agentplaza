//! DIAGNOSTIC ONLY (branch diag/windows-relay, not for main): does a peer's half-close reach a reader on Windows
//! AF_UNIX when another thread writes on the reader's socket while it waits?

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use apz_uds::{UnixListener, UnixStream};

const ROUNDS: u32 = 200;

fn sock(tag: &str, round: u32) -> PathBuf {
    std::env::temp_dir().join(format!(
        "apz-diag-{tag}-{}-{round}.sock",
        std::process::id()
    ))
}

/// What the reader saw before it stopped.
#[derive(Debug)]
enum Seen {
    Eof { bytes: Vec<u8>, after: Duration },
    Error { bytes: Vec<u8>, err: String },
    Stuck { bytes: Vec<u8>, timeouts: u32 },
}

/// One round: the server's reader thread reads (with `timeout`, or blocking when `None`) until EOF; when `send` is
/// set, the server writes "pong" while the reader waits; the client writes "hello", reads "pong" when sent, then
/// half-closes. Returns what the reader saw.
fn round(tag: &str, n: u32, timeout: Option<Duration>, send: bool) -> Seen {
    let path = sock(tag, n);
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).expect("bind");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let mut client = UnixStream::connect(&path).expect("connect");
    let server = loop {
        match listener.accept() {
            Ok((s, _)) => break s,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5))
            }
            Err(e) => panic!("accept: {e}"),
        }
    };
    server.set_nonblocking(false).expect("blocking");
    let mut reader = server.try_clone().expect("clone");
    if let Some(t) = timeout {
        reader.set_read_timeout(Some(t)).expect("timeout");
    }
    let (tx, rx) = mpsc::channel();
    let started = Instant::now();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buf = [0u8; 64];
        let mut timeouts = 0u32;
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if Instant::now() > deadline {
                let _ = tx.send(Seen::Stuck { bytes, timeouts });
                return;
            }
            match reader.read(&mut buf) {
                Ok(0) => {
                    let _ = tx.send(Seen::Eof {
                        bytes,
                        after: started.elapsed(),
                    });
                    return;
                }
                Ok(k) => bytes.extend_from_slice(&buf[..k]),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    timeouts += 1
                }
                Err(e) => {
                    let _ = tx.send(Seen::Error {
                        bytes,
                        err: format!("{e} ({:?})", e.raw_os_error()),
                    });
                    return;
                }
            }
        }
    });
    client.write_all(b"hello").expect("hello");
    if send {
        thread::sleep(Duration::from_millis((n % 4) as u64 * 3));
        let mut writer = server.try_clone().expect("writer");
        writer.write_all(b"pong").expect("pong");
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("t");
        let mut reply = [0u8; 4];
        client.read_exact(&mut reply).expect("the pong");
    }
    thread::sleep(Duration::from_millis((n % 5) as u64 * 2));
    client.shutdown(Shutdown::Write).expect("half close");
    let seen = match rx.recv_timeout(Duration::from_secs(4)) {
        Ok(seen) => seen,
        // A blocking reader that never returned: wake it (shutdown(Read) cancels its recv) and record it as stuck.
        Err(_) => {
            let _ = server.shutdown(Shutdown::Read);
            Seen::Stuck {
                bytes: Vec::new(),
                timeouts: u32::MAX,
            }
        }
    };
    drop((client, server, listener));
    let _ = std::fs::remove_file(&path);
    seen
}

fn run(tag: &str, timeout: Option<Duration>, send: bool) {
    let mut bad = Vec::new();
    for n in 0..ROUNDS {
        match round(tag, n, timeout, send) {
            Seen::Eof { ref bytes, .. } if bytes == b"hello" => {}
            other => bad.push((n, other)),
        }
    }
    eprintln!("DIAG {tag}: {} of {ROUNDS} rounds bad", bad.len());
    for (n, seen) in bad.iter().take(10) {
        eprintln!("DIAG {tag}: round {n}: {seen:?}");
    }
    assert!(bad.is_empty(), "DIAG {tag}: {} bad rounds", bad.len());
}

#[test]
fn diag_uds_polled_no_send() {
    run("polled-nosend", Some(Duration::from_millis(100)), false);
}

#[test]
fn diag_uds_polled_concurrent_send() {
    run("polled-send", Some(Duration::from_millis(100)), true);
}

#[test]
fn diag_uds_blocking_concurrent_send() {
    run("blocking-send", None, true);
}
