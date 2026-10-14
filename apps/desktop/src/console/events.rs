//! The hub's live event stream (`GET /v1/ws`, a WebSocket with the `apz.events.v1` subprotocol) as
//! an iced subscription: each frame's event kind and JSON data. Plain TCP on loopback, pinned TLS
//! to a remote hub (with the client's bearer). One thread owns each hub's socket and fans every
//! frame out, decoded once, to all its subscribers (the top bar and the screens); it reconnects with
//! backoff, and events are dropped when a UI is behind rather than blocking the socket.

use std::hash::{Hash, Hasher};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use apz_http::client::Target;
use apz_http::ws::{self, Limits, Message, Socket};
use iced::Subscription;
use iced::futures::channel::mpsc;
use iced::futures::{SinkExt, Stream, StreamExt};
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
/// Each hub's subscribers: the channel of every live subscription, keyed by the hub's base URL. A hub with
/// subscribers has one socket thread, which ends with the last of them.
static HUBS: Mutex<Vec<(String, Vec<mpsc::Sender<Event>>)>> = Mutex::new(Vec::new());
/// Each hub's connection: whether its socket is up, and the watchers (the top bar) told of each change.
static LINKS: Mutex<Vec<Link>> = Mutex::new(Vec::new());

/// One hub's connection state, by base URL.
struct Link {
    base: String,
    up: bool,
    watchers: Vec<mpsc::Sender<bool>>,
}

/// One event: its name (the bus kind: "transcript", "agent", "delta", …) and data.
#[derive(Debug, Clone)]
pub struct Event {
    pub kind: String,
    pub data: Value,
}

/// The stream for `client`'s hub, as a screen's subscription.
pub fn subscription(client: &Client) -> Subscription<Event> {
    stream(&client.target, "screen")
}

/// The same frames for the shell's top bar, whichever screen is shown.
pub fn top_bar(target: &Target) -> Subscription<Event> {
    stream(target, "top-bar")
}

/// Whether the socket to `target`'s hub is connected, for the top bar's "Live" or "Reconnecting…":
/// the state now, then each change. It is down until the first connection.
pub fn link(target: &Target) -> Subscription<bool> {
    Subscription::run_with(Feed(target.clone(), "link"), |feed: &Feed| {
        watch(feed.0.clone())
    })
}

fn stream(target: &Target, tag: &'static str) -> Subscription<Event> {
    Subscription::run_with(Feed(target.clone(), tag), |feed: &Feed| {
        open(feed.0.clone())
    })
}

/// The subscription's data: the target and a tag, hashed by its base URL and the tag. A screen's stream and
/// the top bar's differ by tag, so both run at once.
#[derive(Clone)]
struct Feed(Target, &'static str);

impl Hash for Feed {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.base.hash(state);
        self.1.hash(state);
    }
}

fn open(target: Target) -> impl Stream<Item = Event> {
    iced::stream::channel(BUFFER, move |mut out: mpsc::Sender<Event>| async move {
        let mut rx = subscribe(&target);
        while let Some(event) = rx.next().await {
            if out.send(event).await.is_err() {
                break;
            }
        }
    })
}

/// Registers a subscriber on `target`'s hub, starting the hub's socket thread when none runs.
fn subscribe(target: &Target) -> mpsc::Receiver<Event> {
    let (tx, rx) = mpsc::channel::<Event>(BUFFER);
    let mut hubs = hubs();
    match hubs.iter_mut().find(|(base, _)| *base == target.base) {
        Some((_, subs)) => subs.push(tx),
        None => {
            hubs.push((target.base.clone(), vec![tx]));
            let target = target.clone();
            std::thread::spawn(move || run(target));
        }
    }
    rx
}

fn hubs() -> MutexGuard<'static, Vec<(String, Vec<mpsc::Sender<Event>>)>> {
    HUBS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn links() -> MutexGuard<'static, Vec<Link>> {
    LINKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `base`'s connection entry, made down when it is first asked for.
fn link_of<'a>(links: &'a mut Vec<Link>, base: &str) -> &'a mut Link {
    let i = match links.iter().position(|link| link.base == base) {
        Some(i) => i,
        None => {
            links.push(Link {
                base: base.to_string(),
                up: false,
                watchers: Vec::new(),
            });
            links.len() - 1
        }
    };
    &mut links[i]
}

/// Records whether `base`'s socket is up and tells each watcher; a watcher that is gone is dropped.
fn set_link(base: &str, up: bool) {
    let mut links = links();
    let link = link_of(&mut links, base);
    link.up = up;
    link.watchers.retain_mut(|tx| match tx.try_send(up) {
        Ok(()) => true,
        Err(e) => !e.is_disconnected(),
    });
}

/// The link's state as a stream: the current state first, then every change, for one watcher.
fn watch(target: Target) -> impl Stream<Item = bool> {
    iced::stream::channel(BUFFER, move |mut out: mpsc::Sender<bool>| async move {
        let (tx, mut rx) = mpsc::channel::<bool>(BUFFER);
        let up = {
            let mut links = links();
            let link = link_of(&mut links, &target.base);
            link.watchers.push(tx);
            link.up
        };
        if out.send(up).await.is_err() {
            return;
        }
        while let Some(state) = rx.next().await {
            if out.send(state).await.is_err() {
                break;
            }
        }
    })
}

/// The socket's thread: connects, reads frames, and reconnects with backoff until the stream ends.
fn run(target: Target) {
    let base = target.base.clone();
    let mut backoff = MIN_BACKOFF;
    while wanted(&base) {
        if let Ok(mut socket) = ws::connect(
            &target,
            PATH,
            Some(PROTOCOL),
            CONNECT_TIMEOUT,
            Limits::default(),
        ) {
            set_link(&base, true);
            let connected = Instant::now();
            read(&mut socket, &base);
            set_link(&base, false);
            if connected.elapsed() >= RESET_AFTER {
                backoff = MIN_BACKOFF;
            }
        }
        if !pause(jittered(backoff), &base) {
            break;
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
    set_link(&base, false);
}

/// Reads frames until the socket fails or the stream ends.
fn read(socket: &mut Socket, base: &str) {
    while wanted(base) {
        match socket.recv(RECV_WAIT) {
            Ok(Some(Message::Text(text))) => {
                if let Some(event) = parse(&text) {
                    publish(base, &event);
                }
            }
            Ok(Some(Message::Binary(_))) | Ok(None) => {}
            Err(_) => return,
        }
    }
}

/// Whether `base`'s hub still has a subscriber. When none is left, its entry goes, and so does its socket thread.
fn wanted(base: &str) -> bool {
    let mut hubs = hubs();
    let Some(i) = hubs.iter().position(|(b, _)| b == base) else {
        return false;
    };
    hubs[i].1.retain(|tx| !tx.is_closed());
    if hubs[i].1.is_empty() {
        hubs.swap_remove(i);
        return false;
    }
    true
}

/// Sends one decoded frame to every subscriber of `base`'s hub.
fn publish(base: &str, event: &Event) {
    let mut hubs = hubs();
    if let Some((_, subs)) = hubs.iter_mut().find(|(b, _)| b == base) {
        fan_out(subs, event);
    }
}

/// Sends `event` to each subscriber: a full channel drops it (that UI is behind), a closed one is removed.
fn fan_out(subs: &mut Vec<mpsc::Sender<Event>>, event: &Event) {
    subs.retain_mut(|tx| match tx.try_send(event.clone()) {
        Ok(()) => true,
        Err(e) => !e.is_disconnected(),
    });
}

/// One frame, `{"event": kind, "data": data}`; anything else is skipped.
fn parse(text: &str) -> Option<Event> {
    let mut frame: Value = serde_json::from_str(text).ok()?;
    let kind = frame.get("event")?.as_str()?.to_string();
    let data = frame.get_mut("data")?.take();
    Some(Event { kind, data })
}

/// Sleeps `total` in short steps; false when the stream ended meanwhile.
fn pause(total: Duration, base: &str) -> bool {
    let end = Instant::now() + total;
    while wanted(base) {
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
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let unit = f64::from(nanos) / 1e9;
    base.mul_f64(0.75 + 0.5 * unit).min(MAX_BACKOFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_parse_keeps_the_kind_and_data_of_a_frame() {
        let plaza = parse(r#"{"event":"plaza","data":{"paused":true}}"#).unwrap();
        assert_eq!(plaza.kind, "plaza");
        assert_eq!(plaza.data["paused"], true);
        assert!(parse(r#"{"data":{}}"#).is_none());
    }

    #[test]
    fn the_link_watchers_hear_each_change_and_a_gone_one_is_dropped() {
        let base = "https://link-watchers.test:7700";
        let (tx, mut rx) = mpsc::channel::<bool>(4);
        let (gone, gone_rx) = mpsc::channel::<bool>(4);
        drop(gone_rx);
        {
            let mut links = links();
            let link = link_of(&mut links, base);
            assert!(!link.up, "a hub starts down");
            link.watchers.push(tx);
            link.watchers.push(gone);
        }
        set_link(base, true);
        set_link(base, false);
        assert!(rx.try_recv().unwrap());
        assert!(!rx.try_recv().unwrap());
        let links = links();
        let link = links.iter().find(|link| link.base == base).unwrap();
        assert_eq!(
            link.watchers.len(),
            1,
            "the gone watcher is removed, the live one stays"
        );
        assert!(!link.up);
    }

    #[test]
    fn events_fan_out_reaches_every_live_subscriber_and_prunes_the_closed_one() {
        let (a, mut ra) = mpsc::channel::<Event>(4);
        let (b, rb) = mpsc::channel::<Event>(4);
        let (c, mut rc) = mpsc::channel::<Event>(0); // one slot: a sender's capacity is buffer + 1
        drop(rb);
        let mut subs = vec![a, b, c];
        let frame = parse(r#"{"event":"agent","data":{"cid":200}}"#).unwrap();
        fan_out(&mut subs, &frame);
        fan_out(&mut subs, &frame);
        assert_eq!(
            subs.len(),
            2,
            "the closed subscriber is removed, the full one stays"
        );
        for _ in 0..2 {
            let got = ra.try_recv().unwrap();
            assert_eq!(got.kind, "agent");
            assert_eq!(got.data["cid"], 200);
        }
        assert_eq!(rc.try_recv().unwrap().kind, "agent");
        assert!(
            rc.try_recv().is_err(),
            "a full subscriber drops the frame rather than blocking"
        );
    }
}
