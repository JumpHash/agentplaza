//! The hub's live event stream (`GET /v1/ws`, a WebSocket with the `apz.events.v1` subprotocol) as
//! an iced subscription: each frame's event kind and JSON data. Plain TCP on loopback, pinned TLS
//! to a remote hub (with the client's bearer). A thread owns the socket and reconnects with
//! backoff; events are dropped when the UI is behind rather than blocking the socket.

use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use apz_http::client::Target;
use apz_http::ws::{self, Limits, Message, Socket};
use iced::futures::channel::mpsc;
use iced::futures::{SinkExt, Stream, StreamExt};
use iced::Subscription;
use serde_json::Value;

use super::client::Client;

const PATH: &str = "/v1/ws";
const PROTOCOL: &str = "apz.events.v1";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long one read may wait, so the thread notices when the subscription has ended.
const RECV_WAIT: Duration = Duration::from_millis(500);
const PAUSE_STEP: Duration = Duration::from_millis(200);
const MIN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// A connection that stayed up this long resets the backoff.
const RESET_AFTER: Duration = Duration::from_secs(60);
/// Events held between the thread and the UI; more are dropped.
const BUFFER: usize = 256;

/// One event: its name (the bus kind: "transcript", "agent", "delta", …) and data.
#[derive(Debug, Clone)]
pub struct Event {
    pub kind: String,
    pub data: Value,
}

/// The stream for `client`'s hub. Its identity is the hub's base URL.
pub fn subscription(client: &Client) -> Subscription<Event> {
    Subscription::run_with(Feed(client.target.clone()), |feed: &Feed| open(feed.0.clone()))
}

/// The subscription's data: the target, hashed by its base URL alone.
#[derive(Clone)]
struct Feed(Target);

impl Hash for Feed {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.base.hash(state);
    }
}

fn open(target: Target) -> impl Stream<Item = Event> {
    iced::stream::channel(BUFFER, move |mut out: mpsc::Sender<Event>| async move {
        let (tx, mut rx) = mpsc::channel::<Event>(BUFFER);
        std::thread::spawn(move || run(target, tx));
        while let Some(event) = rx.next().await {
            if out.send(event).await.is_err() {
                break;
            }
        }
    })
}

/// The socket's thread: connects, reads frames, and reconnects with backoff until the stream ends.
fn run(target: Target, mut tx: mpsc::Sender<Event>) {
    let mut backoff = MIN_BACKOFF;
    while !tx.is_closed() {
        if let Ok(mut socket) = ws::connect(&target, PATH, Some(PROTOCOL), CONNECT_TIMEOUT, Limits::default()) {
            let connected = Instant::now();
            read(&mut socket, &mut tx);
            if connected.elapsed() >= RESET_AFTER {
                backoff = MIN_BACKOFF;
            }
        }
        if !pause(jittered(backoff), &tx) {
            return;
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Reads frames until the socket fails or the stream ends.
fn read(socket: &mut Socket, tx: &mut mpsc::Sender<Event>) {
    while !tx.is_closed() {
        match socket.recv(RECV_WAIT) {
            Ok(Some(Message::Text(text))) => {
                if let Some(event) = parse(&text) {
                    match tx.try_send(event) {
                        Ok(()) => {}
                        // The UI is behind: drop the event rather than block the socket.
                        Err(e) if e.is_full() => {}
                        Err(_) => return,
                    }
                }
            }
            Ok(Some(Message::Binary(_))) | Ok(None) => {}
            Err(_) => return,
        }
    }
}

/// One frame, `{"event": kind, "data": data}`; anything else is skipped.
fn parse(text: &str) -> Option<Event> {
    let mut frame: Value = serde_json::from_str(text).ok()?;
    let kind = frame.get("event")?.as_str()?.to_string();
    let data = frame.get_mut("data")?.take();
    Some(Event { kind, data })
}

/// Sleeps `total` in short steps; false when the stream ended meanwhile.
fn pause(total: Duration, tx: &mpsc::Sender<Event>) -> bool {
    let end = Instant::now() + total;
    while !tx.is_closed() {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        std::thread::sleep(left.min(PAUSE_STEP));
    }
    false
}

/// The backoff with ±25% jitter (from the clock's sub-second part, no RNG), at most `MAX_BACKOFF`.
fn jittered(base: Duration) -> Duration {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let unit = f64::from(nanos) / 1e9;
    base.mul_f64(0.75 + 0.5 * unit).min(MAX_BACKOFF)
}
