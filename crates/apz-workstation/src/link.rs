//! The hub link: dial `WS /v1/stations/link`, say hello, serve the hub's requests, and redial when it drops.
//!
//! The socket is owned by one thread (this module's [`serve`]), because a WebSocket is read and written by the
//! same caller. Answers and events from the other threads queue on the [`Uplink`] and this thread writes them
//! between reads, so an answer waits at most one poll (20 ms).
//!
//! Reconnects back off from 1 s to 30 s. A `revoked` notice ends the daemon; `drain` takes no new requests;
//! STOP ALL refuses everything but its own lifting (see [`Daemon::admit`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use apz_http::client::Target;
use apz_http::ws::{self, Limits, Message, Socket};
use apz_station::{HubFrame, LINK_PATH, Notice, SUBPROTOCOL, StationFrame};

use crate::daemon::Daemon;
use crate::state::{self, Record, Status};

/// How long one dial may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The longest one socket read waits, which bounds how late an answer can be written.
const POLL: Duration = Duration::from_millis(20);
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// The WebSocket limits: a screenshot answer can be large, so messages may be up to 32 MiB.
pub fn limits() -> Limits {
    Limits {
        max_frame: 4 << 20,
        max_message: 32 << 20,
        ping_every: Duration::from_secs(20),
        dead_after: Duration::from_secs(45),
    }
}

/// Why one connection ended.
enum Ended {
    /// The hub revoked the station: the daemon stops.
    Revoked,
    /// The link broke; the reason is for the log.
    Lost(String),
}

/// Keeps the station linked until `stop` is set (`Ok`) or the hub revokes it (`Err`).
pub fn run(daemon: &Daemon, record: &Record, stop: &AtomicBool) -> Result<(), String> {
    let target = Target {
        base: record.hub.clone(),
        pin: record.pin_bytes()?,
        bearer: Some(record.token.clone()),
        headers: Vec::new(),
    };
    let mut backoff = BACKOFF_MIN;
    loop {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        match ws::connect(
            &target,
            LINK_PATH,
            Some(SUBPROTOCOL),
            CONNECT_TIMEOUT,
            limits(),
        ) {
            Ok(socket) => {
                backoff = BACKOFF_MIN;
                match serve(daemon, record, socket, stop) {
                    Ok(()) => return Ok(()),
                    Err(Ended::Revoked) => {
                        report(daemon, false, Some("the hub revoked this station"));
                        daemon
                            .shared
                            .log
                            .error("the hub revoked this station; stopping");
                        return Err("the hub revoked this station".to_string());
                    }
                    Err(Ended::Lost(reason)) => {
                        daemon.shared.log.warn(format!("link lost: {reason}"));
                        report(daemon, false, Some(&reason));
                    }
                }
            }
            Err(err) => {
                daemon
                    .shared
                    .log
                    .warn(format!("cannot reach the hub: {err}"));
                report(daemon, false, Some(&err.to_string()));
            }
        }
        sleep_unless_stopped(backoff, stop);
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// One connection: hello first, then the request loop until the link ends.
fn serve(
    daemon: &Daemon,
    record: &Record,
    mut socket: Socket,
    stop: &AtomicBool,
) -> Result<(), Ended> {
    let hello = StationFrame::Hello {
        station: record.station.clone(),
        version: apz_os::version().to_string(),
        facts: daemon.facts_now(),
    };
    send(&mut socket, &hello).map_err(Ended::Lost)?;
    let (tx, rx) = mpsc::channel();
    let generation = daemon.shared.uplink.attach(tx);
    daemon
        .shared
        .log
        .info(format!("linked as station {}", record.station));
    report(daemon, true, None);

    let outcome = pump(daemon, &mut socket, &rx, generation, stop);
    daemon.shared.uplink.detach();
    socket.close(1000, "");
    outcome
}

/// Reads the hub's frames and writes the queued answers, until the link ends or stop is set.
fn pump(
    daemon: &Daemon,
    socket: &mut Socket,
    rx: &mpsc::Receiver<StationFrame>,
    generation: u64,
    stop: &AtomicBool,
) -> Result<(), Ended> {
    let mut draining = false;
    loop {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        while let Ok(frame) = rx.try_recv() {
            send(socket, &frame).map_err(Ended::Lost)?;
        }
        match socket.recv(POLL) {
            Ok(Some(Message::Text(text))) => match serde_json::from_str::<HubFrame>(&text) {
                Ok(HubFrame::Request { id, op }) => match daemon.admit(&op, draining) {
                    Ok(()) => daemon.submit(generation, id, op),
                    Err(refusal) => daemon.shared.uplink.respond(generation, id, Err(refusal)),
                },
                Ok(HubFrame::Notify { notice }) => match notice {
                    Notice::Revoked => return Err(Ended::Revoked),
                    Notice::Drain => {
                        draining = true;
                        daemon.shared.log.info("the hub is draining this station");
                    }
                    Notice::SettingsChanged => {}
                },
                // The text itself is not logged: it is the hub's data.
                Err(_) => daemon
                    .shared
                    .log
                    .warn("a frame from the hub is not one this daemon understands"),
            },
            Ok(Some(Message::Binary(_))) => daemon
                .shared
                .log
                .warn("a binary frame from the hub is ignored"),
            Ok(None) => {}
            Err(err) => return Err(Ended::Lost(err.to_string())),
        }
    }
}

fn send(socket: &mut Socket, frame: &StationFrame) -> Result<(), String> {
    let text = serde_json::to_string(frame).map_err(|e| e.to_string())?;
    socket.send(&Message::Text(text)).map_err(|e| e.to_string())
}

/// Writes the link's state to the status file for `status`. Failures are not fatal.
fn report(daemon: &Daemon, connected: bool, error: Option<&str>) {
    let status = Status {
        pid: std::process::id(),
        version: apz_os::version().to_string(),
        backend: daemon.shared.backend.name().to_string(),
        connected,
        since: unix_now(),
        last_error: error.map(str::to_string),
    };
    let _ = state::save_status(&daemon.shared.state_dir, &status);
}

/// Sleeps for `total`, waking early when stop is set.
fn sleep_unless_stopped(total: Duration, stop: &AtomicBool) {
    let until = Instant::now() + total;
    while Instant::now() < until && !stop.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(50));
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
