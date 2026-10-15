//! The live view of a workstation (docs/design/workstations.md section 7). `WS /v1/stations/{id}/screen` streams a
//! session's frames to the owner, and `GET /v1/stations/{id}/thumbnail` gives one small picture for the Machines and
//! Floor pages. Both are owner-only (the gate in `hub_auth`).
//!
//!   WS  /v1/stations/{id}/screen?session=<id|shared>&fps=<1..15>&scale=<0.1..1>   subprotocol apz.station.screen.v1
//!   GET /v1/stations/{id}/thumbnail?session=<id|shared>                            → one JPEG, scaled 0.2, cached 5 s
//!
//! A frame is one binary message: a 4-byte big-endian length, the header `{"w","h","format","ts","seq"}` (`format` is
//! `png` or `jpeg`, `ts` the hub's time in ms), then the image. The hub's text messages are `{"t":"started",
//! "session":N}` once the stream runs, `{"t":"ended","reason":...}` when the session ends (the socket then closes
//! 1000), `{"t":"error","error":...}`, and `{"t":"pong"}`. The client sends `{"t":"ping"}`; anything else is answered
//! with an error and the socket stays open.
//!
//! The viewers of one session share one stream on the station. It runs at the highest fps and the smallest scale the
//! viewers ask for, and stops when the last one closes. Each viewer receives at most its own fps. `session=shared` is
//! the station's shared session, the owner's own screen: the first view opens it and the last one closes it. A
//! thumbnail with no session names the session of an active lease.

use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use apz_http::ws::{self, Limits, Message, Origins, Socket};
use apz_http::{Request, Response, Router};
use apz_station::{Event, ImageFormat, Op, SessionId, SessionKind};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use super::ops::{OPEN_TIMEOUT, RPC_TIMEOUT};
use crate::events::Slots;
use crate::state::App;

/// The subprotocol a live view asks for.
pub const PROTOCOL: &str = "apz.station.screen.v1";
/// The agent a station's shared session is opened for: none, since it is the owner's own screen. The daemon's
/// contract for shared sessions names no agent, so the hub passes 0.
const SHARED_AGENT: u64 = 0;
const DEFAULT_FPS: u8 = 5;
const MAX_FPS: u8 = 15;
const DEFAULT_SCALE: f32 = 0.5;
/// How long a thumbnail is kept, in ms.
pub const THUMB_TTL: u64 = 5_000;
/// The scale a thumbnail is taken at.
const THUMB_SCALE: f32 = 0.2;
/// How long a view waits for a client message between batches of frames.
const POLL: Duration = Duration::from_millis(50);
/// The socket limits of a live view and of a control socket: a frame is one message, so a message may be large.
pub const LIMITS: Limits = Limits {
    max_frame: 16 << 20,
    max_message: 16 << 20,
    ping_every: Duration::from_secs(20),
    dead_after: Duration::from_secs(45),
};
/// The takeover and view sockets open now, per peer and in all (the event socket's caps, a pool of their own).
pub static SOCKETS: Slots = Slots::new();

/// Which session a view names: one the daemon opened, or the station's shared session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    Session(SessionId),
    Shared,
}

/// What one viewer asks of the stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Want {
    pub fps: u8,
    pub scale: f32,
}

/// A live view's query: the session and what it asks of the stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub pick: Pick,
    pub want: Want,
}

/// The session a query names: a number, or `shared`.
pub fn parse_pick(session: Option<&str>) -> Result<Pick, String> {
    match session {
        None | Some("") => Err("name the session: session=<id> or session=shared".into()),
        Some("shared") => Ok(Pick::Shared),
        Some(raw) => raw
            .parse::<SessionId>()
            .map(Pick::Session)
            .map_err(|_| "session is a session id or shared".to_string()),
    }
}

/// A live view's query, checked: the session, `fps` from 1 to 15 (5 by default) and `scale` from 0.1 to 1 (0.5 by
/// default).
pub fn parse_view(
    session: Option<&str>,
    fps: Option<&str>,
    scale: Option<&str>,
) -> Result<View, String> {
    let pick = parse_pick(session)?;
    let fps = match fps {
        None => DEFAULT_FPS,
        Some(raw) => raw
            .parse::<u8>()
            .ok()
            .filter(|f| (1..=MAX_FPS).contains(f))
            .ok_or("fps is a whole number from 1 to 15")?,
    };
    let scale = match scale {
        None => DEFAULT_SCALE,
        Some(raw) => raw
            .parse::<f32>()
            .ok()
            .filter(|s| s.is_finite() && (0.1..=1.0).contains(s))
            .ok_or("scale is a number from 0.1 to 1")?,
    };
    Ok(View {
        pick,
        want: Want { fps, scale },
    })
}

/// What the views ask of the station. The live hub is [`Link`]; the tests use a script.
pub trait Screens {
    fn open_shared(&self, station: &str) -> Result<SessionId, String>;
    fn close(&self, station: &str, session: SessionId) -> Result<(), String>;
    /// Starts (`on`) or stops the session's stream, at `want`.
    fn stream(&self, station: &str, session: SessionId, on: bool, want: Want)
    -> Result<(), String>;
    fn screenshot(
        &self,
        station: &str,
        session: SessionId,
        scale: f32,
    ) -> Result<(ImageFormat, Vec<u8>), String>;
}

/// The live hub's side: the station's link.
pub struct Link<'a>(pub &'a App);

impl Link<'_> {
    fn request(&self, station: &str, op: Op, timeout: Duration) -> Result<Value, String> {
        let link = self
            .0
            .stations
            .live(station)
            .ok_or_else(|| "the workstation is not linked".to_string())?;
        link.request(op, timeout)
    }
}

impl Screens for Link<'_> {
    fn open_shared(&self, station: &str) -> Result<SessionId, String> {
        let answer = self.request(
            station,
            Op::SessionOpen {
                agent: SHARED_AGENT,
                kind: SessionKind::Shared,
            },
            OPEN_TIMEOUT,
        )?;
        answer["session"]
            .as_u64()
            .ok_or_else(|| "the workstation's answer names no session".to_string())
    }

    fn close(&self, station: &str, session: SessionId) -> Result<(), String> {
        self.request(station, Op::SessionClose { session }, RPC_TIMEOUT)
            .map(|_| ())
    }

    fn stream(
        &self,
        station: &str,
        session: SessionId,
        on: bool,
        want: Want,
    ) -> Result<(), String> {
        let op = Op::Stream {
            session,
            fps: want.fps,
            on,
            scale: on.then_some(want.scale),
        };
        self.request(station, op, RPC_TIMEOUT).map(|_| ())
    }

    fn screenshot(
        &self,
        station: &str,
        session: SessionId,
        scale: f32,
    ) -> Result<(ImageFormat, Vec<u8>), String> {
        let op = Op::Screenshot {
            session,
            scale,
            grid: false,
            format: ImageFormat::Jpeg,
        };
        let answer = self.request(station, op, RPC_TIMEOUT)?;
        let format = serde_json::from_value::<ImageFormat>(answer["format"].clone())
            .map_err(|e| format!("the screenshot has no known format: {e}"))?;
        let bytes = STANDARD
            .decode(answer["data_b64"].as_str().unwrap_or_default())
            .map_err(|e| format!("the screenshot is not base64: {e}"))?;
        Ok((format, bytes))
    }
}

type Key = (String, SessionId);

/// One session's stream on a station: its viewers, whether it is running and at what, and whether the hub opened it
/// as the shared session.
#[derive(Default)]
struct Feed {
    shared: bool,
    viewers: BTreeMap<u64, Want>,
    streaming: Option<Want>,
}

struct Thumb {
    at: u64,
    format: ImageFormat,
    bytes: Vec<u8>,
}

/// The live views: the feeds of every session being watched, and the thumbnails.
#[derive(Default)]
pub struct Live {
    feeds: Mutex<BTreeMap<Key, Feed>>,
    thumbs: Mutex<BTreeMap<Key, Thumb>>,
    viewers: AtomicU64,
}

impl Live {
    fn feeds(&self) -> MutexGuard<'_, BTreeMap<Key, Feed>> {
        self.feeds.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn thumbs(&self) -> MutexGuard<'_, BTreeMap<Key, Thumb>> {
        self.thumbs.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A new viewer's id.
    pub fn next_viewer(&self) -> u64 {
        self.viewers.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// The station's shared session, while a view has it open.
    pub fn shared_session(&self, station: &str) -> Option<SessionId> {
        self.feeds()
            .iter()
            .find(|((s, _), feed)| s.as_str() == station && feed.shared)
            .map(|((_, session), _)| *session)
    }

    /// Joins `viewer` to the stream of `pick` on `station`, starting the stream when it is the first. The answer is
    /// the session the view watches. Nothing is left behind when the stream cannot start.
    pub fn join(
        &self,
        screens: &dyn Screens,
        station: &str,
        pick: Pick,
        viewer: u64,
        want: Want,
    ) -> Result<SessionId, String> {
        let mut feeds = self.feeds();
        let key = match pick {
            Pick::Session(session) => (station.to_string(), session),
            Pick::Shared => match feeds
                .iter()
                .find(|((s, _), feed)| s.as_str() == station && feed.shared)
            {
                Some((key, _)) => key.clone(),
                None => {
                    let key = (station.to_string(), screens.open_shared(station)?);
                    feeds.insert(
                        key.clone(),
                        Feed {
                            shared: true,
                            ..Feed::default()
                        },
                    );
                    key
                }
            },
        };
        feeds
            .entry(key.clone())
            .or_default()
            .viewers
            .insert(viewer, want);
        match settle(screens, &mut feeds, &key) {
            Ok(()) => Ok(key.1),
            Err(reason) => {
                if let Some(feed) = feeds.get_mut(&key) {
                    feed.viewers.remove(&viewer);
                }
                let _ = settle(screens, &mut feeds, &key);
                Err(reason)
            }
        }
    }

    /// Takes `viewer` off the stream of `session`. The last viewer stops the stream, and closes a shared session.
    pub fn leave(&self, screens: &dyn Screens, station: &str, session: SessionId, viewer: u64) {
        let key = (station.to_string(), session);
        let mut feeds = self.feeds();
        if let Some(feed) = feeds.get_mut(&key) {
            feed.viewers.remove(&viewer);
        }
        if let Err(reason) = settle(screens, &mut feeds, &key) {
            eprintln!("apid: workstation {station}: its stream did not change: {reason}");
        }
    }

    /// One thumbnail of `session`: the cached one while it is fresh, else a new screenshot at [`THUMB_SCALE`].
    pub fn thumbnail(
        &self,
        screens: &dyn Screens,
        station: &str,
        session: SessionId,
        now: u64,
    ) -> Result<(ImageFormat, Vec<u8>), String> {
        let key = (station.to_string(), session);
        if let Some(thumb) = self
            .thumbs()
            .get(&key)
            .filter(|t| now.saturating_sub(t.at) < THUMB_TTL)
        {
            return Ok((thumb.format, thumb.bytes.clone()));
        }
        let (format, bytes) = screens.screenshot(station, session, THUMB_SCALE)?;
        let mut thumbs = self.thumbs();
        thumbs.retain(|_, t| now.saturating_sub(t.at) < THUMB_TTL);
        thumbs.insert(
            key,
            Thumb {
                at: now,
                format,
                bytes: bytes.clone(),
            },
        );
        Ok((format, bytes))
    }
}

/// The stream the viewers of a feed want: the highest fps and the smallest scale. None when nobody watches.
fn wanted(viewers: &BTreeMap<u64, Want>) -> Option<Want> {
    viewers.values().copied().reduce(|a, b| Want {
        fps: a.fps.max(b.fps),
        scale: a.scale.min(b.scale),
    })
}

/// Makes the station's stream of `key` match its viewers: started or re-tuned, or, with no viewer left, stopped and
/// removed (and a shared session closed).
fn settle(screens: &dyn Screens, feeds: &mut BTreeMap<Key, Feed>, key: &Key) -> Result<(), String> {
    let Some(feed) = feeds.get(key) else {
        return Ok(());
    };
    let (streaming, shared, want) = (feed.streaming, feed.shared, wanted(&feed.viewers));
    let (station, session) = (key.0.as_str(), key.1);
    match want {
        None => {
            feeds.remove(key);
            if let Some(last) = streaming
                && let Err(reason) = screens.stream(station, session, false, last)
            {
                eprintln!("apid: workstation {station}: its stream did not stop: {reason}");
            }
            if shared && let Err(reason) = screens.close(station, session) {
                eprintln!(
                    "apid: workstation {station}: its shared session did not close: {reason}"
                );
            }
            Ok(())
        }
        Some(want) if streaming == Some(want) => Ok(()),
        Some(want) => {
            screens.stream(station, session, true, want)?;
            if let Some(feed) = feeds.get_mut(key) {
                feed.streaming = Some(want);
            }
            Ok(())
        }
    }
}

/// Whether a frame may go to a viewer that asked for `fps`, given the time of the last one it got: at most one per
/// 1000 / fps ms.
pub fn due(last_ms: Option<u64>, now: u64, fps: u8) -> bool {
    match last_ms {
        None => true,
        Some(last) => now.saturating_sub(last).saturating_mul(u64::from(fps)) >= 1000,
    }
}

/// One frame's binary message: a 4-byte big-endian header length, the header, then the image.
pub fn frame_message(header: &Value, image: &[u8]) -> Vec<u8> {
    let head = header.to_string();
    let mut out = Vec::with_capacity(4 + head.len() + image.len());
    out.extend_from_slice(&(head.len() as u32).to_be_bytes());
    out.extend_from_slice(head.as_bytes());
    out.extend_from_slice(image);
    out
}

/// The socket as the views use it. The live socket implements it; the tests use a script.
pub(super) trait Wire {
    fn recv(&mut self, wait: Duration) -> io::Result<Option<Message>>;
    fn send(&mut self, message: &Message) -> io::Result<()>;
    fn close(&mut self, code: u16, reason: &str);
}

impl Wire for Socket {
    fn recv(&mut self, wait: Duration) -> io::Result<Option<Message>> {
        Socket::recv(self, wait)
    }

    fn send(&mut self, message: &Message) -> io::Result<()> {
        Socket::send(self, message)
    }

    fn close(&mut self, code: u16, reason: &str) {
        Socket::close(self, code, reason);
    }
}

/// Sends one JSON text message; false when the socket is gone.
pub(super) fn send_text(socket: &mut dyn Wire, value: &Value) -> bool {
    socket.send(&Message::Text(value.to_string())).is_ok()
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/stations/{id}/screen", |app, req, p| {
            screen_route(app, req, p.get("id"))
        })
        .get("/v1/stations/{id}/thumbnail", |app, req, p| {
            thumbnail_route(app, req, p.get("id"))
        })
}

fn screen_route(app: &App, req: &Request, id: &str) -> Response {
    let view = match parse_view(
        req.query_param("session"),
        req.query_param("fps"),
        req.query_param("scale"),
    ) {
        Ok(view) => view,
        Err(reason) => return Response::error(400, &reason),
    };
    let Some(station) = app.stations.station(id) else {
        return Response::error(404, "no such workstation");
    };
    if station.revoked {
        return Response::error(409, "this workstation was revoked");
    }
    let Some(link) = app.stations.live(id) else {
        return Response::error(409, "this workstation is offline");
    };
    let Some(slot) = SOCKETS.take(req.peer) else {
        return Response::error(
            429,
            "too many open live views and takeover sockets; close one first",
        );
    };
    // Subscribed before the stream starts, so no frame of it is missed.
    let events = link.subscribe();
    let weak = app.handle();
    let station_id = id.to_string();
    ws::accept(
        req,
        Origins::SameHost,
        Some(PROTOCOL),
        LIMITS,
        move |mut socket| {
            let _slot = slot;
            let Some(app) = weak.upgrade() else {
                socket.close(1001, "the hub is shutting down");
                return;
            };
            serve(&app, &station_id, view, &events, &mut socket);
        },
    )
}

/// Serves one view until it ends: joins the stream, relays the frames, and leaves the stream when the view closes.
fn serve(app: &App, station: &str, view: View, events: &Receiver<Event>, socket: &mut dyn Wire) {
    let viewer = app.stations.screen.next_viewer();
    let screens = Link(app);
    let session = match app
        .stations
        .screen
        .join(&screens, station, view.pick, viewer, view.want)
    {
        Ok(session) => session,
        Err(reason) => {
            send_text(socket, &json!({"t": "error", "error": reason}));
            socket.close(1011, "the stream could not start");
            return;
        }
    };
    if send_text(socket, &json!({"t": "started", "session": session})) {
        watch(events, session, view.want.fps, socket);
    }
    app.stations
        .screen
        .leave(&screens, station, session, viewer);
}

/// Relays the session's frames to the socket, at its fps, until the session ends, the link ends or the client leaves.
fn watch(events: &Receiver<Event>, session: SessionId, fps: u8, socket: &mut dyn Wire) {
    let mut last_sent = None;
    loop {
        loop {
            match events.try_recv() {
                Ok(Event::Frame {
                    session: id,
                    seq,
                    w,
                    h,
                    format,
                    data_b64,
                }) if id == session => {
                    let now = apz_plaza::now_ms();
                    if !due(last_sent, now, fps) {
                        continue;
                    }
                    let Ok(image) = STANDARD.decode(&data_b64) else {
                        continue;
                    };
                    let header = json!({"w": w, "h": h, "format": format, "ts": now, "seq": seq});
                    if socket
                        .send(&Message::Binary(frame_message(&header, &image)))
                        .is_err()
                    {
                        return;
                    }
                    last_sent = Some(now);
                }
                Ok(Event::SessionEnded {
                    session: id,
                    reason,
                }) if id == session => {
                    send_text(socket, &json!({"t": "ended", "reason": reason}));
                    socket.close(1000, "the session ended");
                    return;
                }
                Ok(_) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    socket.close(1011, "the workstation's link ended");
                    return;
                }
            }
        }
        match socket.recv(POLL) {
            Ok(None) => {}
            Ok(Some(Message::Text(text))) => {
                let is_ping = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| v["t"].as_str().map(str::to_string))
                    .as_deref()
                    == Some("ping");
                let reply = if is_ping {
                    json!({"t": "pong"})
                } else {
                    json!({"t": "error", "error": "the live view takes pings only"})
                };
                if !send_text(socket, &reply) {
                    return;
                }
            }
            Ok(Some(Message::Binary(_))) => {
                socket.close(1003, "text frames only");
                return;
            }
            Err(_) => return,
        }
    }
}

fn thumbnail_route(app: &App, req: &Request, id: &str) -> Response {
    if app.stations.station(id).is_none() {
        return Response::error(404, "no such workstation");
    }
    let raw = req.query_param("session");
    let session = match raw {
        // No session named: the session of the first lease that has one.
        None | Some("") => match app
            .stations
            .leases
            .views(id)
            .into_iter()
            .find_map(|v| v.lease.session)
        {
            Some(session) => session,
            None => return Response::error(409, "no screen session is open on this workstation"),
        },
        Some(_) => match parse_pick(raw) {
            Ok(Pick::Session(session)) => session,
            Ok(Pick::Shared) => match app.stations.screen.shared_session(id) {
                Some(session) => session,
                None => return Response::error(409, "no shared view is open on this workstation"),
            },
            Err(reason) => return Response::error(400, &reason),
        },
    };
    match app
        .stations
        .screen
        .thumbnail(&Link(app), id, session, apz_plaza::now_ms())
    {
        Ok((format, bytes)) => Response::new(200, mime(format), bytes),
        Err(reason) => Response::error(409, &reason),
    }
}

fn mime(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "image/png",
        ImageFormat::Jpeg => "image/jpeg",
    }
}

/// Test doubles of the socket: a script of what the client sends, and a record of what the hub sends.
#[cfg(test)]
pub(super) mod script {
    use std::collections::VecDeque;
    use std::io;
    use std::time::Duration;

    use apz_http::Request;
    use apz_http::ws::Message;
    use serde_json::Value;

    use super::Wire;

    /// A WebSocket upgrade request to `path`, from `origin` (none for a client without one), asking for `protocol`.
    pub fn upgrade_request(origin: Option<&str>, protocol: &str, path: &str) -> Request {
        let mut headers = vec![
            ("upgrade".to_string(), "websocket".to_string()),
            ("connection".to_string(), "Upgrade".to_string()),
            ("sec-websocket-version".to_string(), "13".to_string()),
            (
                "sec-websocket-key".to_string(),
                "dGhlIHNhbXBsZSBub25jZQ==".to_string(),
            ),
            ("host".to_string(), "127.0.0.1:7700".to_string()),
            ("sec-websocket-protocol".to_string(), protocol.to_string()),
        ];
        if let Some(origin) = origin {
            headers.push(("origin".to_string(), origin.to_string()));
        }
        Request {
            method: "GET".into(),
            path: path.into(),
            headers,
            ..Request::default()
        }
    }

    #[derive(Default)]
    pub struct Script {
        pub inbound: VecDeque<Message>,
        pub sent: Vec<Message>,
        pub closed: Option<(u16, String)>,
    }

    impl Script {
        /// The text messages the hub sent, parsed.
        pub fn texts(&self) -> Vec<Value> {
            self.sent
                .iter()
                .filter_map(|m| match m {
                    Message::Text(text) => serde_json::from_str(text).ok(),
                    Message::Binary(_) => None,
                })
                .collect()
        }

        /// The binary messages the hub sent.
        pub fn binaries(&self) -> Vec<Vec<u8>> {
            self.sent
                .iter()
                .filter_map(|m| match m {
                    Message::Binary(bytes) => Some(bytes.clone()),
                    Message::Text(_) => None,
                })
                .collect()
        }
    }

    impl Wire for Script {
        /// The next scripted message; once the script is spent the socket is gone (an error).
        fn recv(&mut self, _wait: Duration) -> io::Result<Option<Message>> {
            match self.inbound.pop_front() {
                Some(message) => Ok(Some(message)),
                None => Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
            }
        }

        fn send(&mut self, message: &Message) -> io::Result<()> {
            self.sent.push(message.clone());
            Ok(())
        }

        fn close(&mut self, code: u16, reason: &str) {
            self.closed = Some((code, reason.to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::sync::mpsc;

    use super::script::Script;
    use super::*;

    /// The station's side, recorded: what the views asked, and how many screenshots were taken.
    #[derive(Default)]
    struct Fake {
        log: RefCell<Vec<String>>,
        shots: RefCell<u32>,
        refuse_stream: bool,
    }

    impl Screens for Fake {
        fn open_shared(&self, station: &str) -> Result<SessionId, String> {
            self.log.borrow_mut().push(format!("open_shared {station}"));
            Ok(77)
        }

        fn close(&self, station: &str, session: SessionId) -> Result<(), String> {
            self.log
                .borrow_mut()
                .push(format!("close {station}/{session}"));
            Ok(())
        }

        fn stream(
            &self,
            station: &str,
            session: SessionId,
            on: bool,
            want: Want,
        ) -> Result<(), String> {
            self.log.borrow_mut().push(format!(
                "stream {station}/{session} {on} {} {}",
                want.fps, want.scale
            ));
            if on && self.refuse_stream {
                Err("the station refused the stream".into())
            } else {
                Ok(())
            }
        }

        fn screenshot(
            &self,
            _station: &str,
            _session: SessionId,
            _scale: f32,
        ) -> Result<(ImageFormat, Vec<u8>), String> {
            *self.shots.borrow_mut() += 1;
            Ok((ImageFormat::Jpeg, vec![1, 2, 3]))
        }
    }

    impl Fake {
        fn lines(&self) -> Vec<String> {
            self.log.borrow().clone()
        }
    }

    fn want(fps: u8, scale: f32) -> Want {
        Want { fps, scale }
    }

    #[test]
    fn live_viewers_start_and_stop_one_stream_at_the_shared_best_settings() {
        // The first viewer starts the stream, and the last one stops it.
        {
            let fx = Fake::default();
            let live = Live::default();
            assert_eq!(
                live.join(&fx, "w1", Pick::Session(9), 1, want(2, 1.0)),
                Ok(9)
            );
            assert_eq!(
                live.join(&fx, "w1", Pick::Session(9), 2, want(2, 1.0)),
                Ok(9)
            );
            live.leave(&fx, "w1", 9, 1);
            live.leave(&fx, "w1", 9, 2);
            assert_eq!(
                fx.lines(),
                vec![
                    "stream w1/9 true 2 1".to_string(),
                    "stream w1/9 false 2 1".to_string()
                ],
                "one start and one stop"
            );
            assert!(live.feeds().is_empty());
        }
        // Several viewers share the highest fps and the smallest scale.
        {
            let fx = Fake::default();
            let live = Live::default();
            live.join(&fx, "w1", Pick::Session(9), 1, want(2, 1.0))
                .unwrap();
            live.join(&fx, "w1", Pick::Session(9), 2, want(5, 0.5))
                .unwrap();
            live.leave(&fx, "w1", 9, 2);
            live.leave(&fx, "w1", 9, 1);
            assert_eq!(
                fx.lines(),
                vec![
                    "stream w1/9 true 2 1".to_string(),
                    "stream w1/9 true 5 0.5".to_string(),
                    "stream w1/9 true 2 1".to_string(),
                    "stream w1/9 false 2 1".to_string(),
                ]
            );
        }
    }

    #[test]
    fn the_shared_session_is_opened_once_and_closed_with_its_last_viewer() {
        let fx = Fake::default();
        let live = Live::default();
        assert_eq!(live.join(&fx, "w1", Pick::Shared, 1, want(5, 0.5)), Ok(77));
        assert_eq!(live.join(&fx, "w1", Pick::Shared, 2, want(5, 0.5)), Ok(77));
        assert_eq!(live.shared_session("w1"), Some(77));
        live.leave(&fx, "w1", 77, 1);
        assert_eq!(
            live.shared_session("w1"),
            Some(77),
            "the second view keeps it open"
        );
        live.leave(&fx, "w1", 77, 2);
        assert_eq!(live.shared_session("w1"), None);
        assert_eq!(
            fx.lines()
                .iter()
                .filter(|l| l.starts_with("open_shared"))
                .count(),
            1
        );
        assert_eq!(
            fx.lines()
                .iter()
                .filter(|l| l.starts_with("close w1/77"))
                .count(),
            1
        );
    }

    #[test]
    fn a_stream_that_cannot_start_leaves_no_viewer_behind() {
        let fx = Fake {
            refuse_stream: true,
            ..Fake::default()
        };
        let live = Live::default();
        assert!(
            live.join(&fx, "w1", Pick::Session(9), 1, want(5, 1.0))
                .is_err()
        );
        assert!(live.feeds().is_empty(), "nothing is left to settle later");
        assert_eq!(
            live.join(&fx, "w1", Pick::Shared, 2, want(5, 1.0)),
            Err("the station refused the stream".to_string())
        );
        assert_eq!(
            fx.lines()
                .iter()
                .filter(|l| l.starts_with("close w1/77"))
                .count(),
            1,
            "the shared session it opened is closed"
        );
    }

    #[test]
    fn the_view_query_is_checked() {
        assert_eq!(
            parse_view(Some("9"), None, None),
            Ok(View {
                pick: Pick::Session(9),
                want: Want { fps: 5, scale: 0.5 }
            })
        );
        assert_eq!(
            parse_view(Some("shared"), Some("15"), Some("1")),
            Ok(View {
                pick: Pick::Shared,
                want: Want {
                    fps: 15,
                    scale: 1.0
                }
            })
        );
        assert!(
            parse_view(None, None, None).is_err(),
            "the session is named"
        );
        assert!(parse_view(Some("nine"), None, None).is_err());
        assert!(parse_view(Some("9"), Some("0"), None).is_err());
        assert!(parse_view(Some("9"), Some("16"), None).is_err());
        assert!(parse_view(Some("9"), None, Some("0.05")).is_err());
        assert!(parse_view(Some("9"), None, Some("1.5")).is_err());
        assert!(parse_view(Some("9"), None, Some("NaN")).is_err());
    }

    #[test]
    fn a_view_relays_its_session_frames_and_ends_with_the_session() {
        let (tx, rx) = mpsc::channel();
        let frame = |session: SessionId, seq: u64| Event::Frame {
            session,
            seq,
            w: 4,
            h: 2,
            format: ImageFormat::Jpeg,
            data_b64: STANDARD.encode([1u8, 2, 3]),
        };
        tx.send(frame(8, 1)).unwrap();
        tx.send(frame(9, 1)).unwrap();
        tx.send(Event::SessionEnded {
            session: 9,
            reason: "closed".into(),
        })
        .unwrap();
        let mut socket = Script::default();
        watch(&rx, 9, 5, &mut socket);

        let binaries = socket.binaries();
        assert_eq!(
            binaries.len(),
            1,
            "another session's frame is not this view's"
        );
        let len = u32::from_be_bytes([
            binaries[0][0],
            binaries[0][1],
            binaries[0][2],
            binaries[0][3],
        ]) as usize;
        let head: Value = serde_json::from_slice(&binaries[0][4..4 + len]).unwrap();
        assert_eq!(
            (
                head["w"].as_u64(),
                head["h"].as_u64(),
                head["format"].as_str(),
                head["seq"].as_u64()
            ),
            (Some(4), Some(2), Some("jpeg"), Some(1))
        );
        assert_eq!(&binaries[0][4 + len..], &[1, 2, 3]);
        assert!(
            socket
                .texts()
                .contains(&json!({"t": "ended", "reason": "closed"}))
        );
        assert_eq!(socket.closed, Some((1000, "the session ended".to_string())));
    }

    #[test]
    fn a_view_whose_link_ends_closes_with_1011_and_a_binary_client_frame_closes_it() {
        let (tx, rx) = mpsc::channel::<Event>();
        drop(tx);
        let mut socket = Script::default();
        watch(&rx, 9, 5, &mut socket);
        assert_eq!(
            socket.closed,
            Some((1011, "the workstation's link ended".to_string()))
        );

        let (_tx, rx) = mpsc::channel::<Event>();
        let mut socket = Script::default();
        socket.inbound.push_back(Message::Binary(vec![1]));
        watch(&rx, 9, 5, &mut socket);
        assert_eq!(socket.closed, Some((1003, "text frames only".to_string())));
    }

    #[test]
    fn the_view_socket_asks_for_its_subprotocol_and_refuses_other_origins() {
        use super::script::upgrade_request;
        let path = "/v1/stations/w1/screen";
        let upgrade = |origin, protocol| {
            ws::accept(
                &upgrade_request(origin, protocol, path),
                Origins::SameHost,
                Some(PROTOCOL),
                LIMITS,
                |_| {},
            )
            .status
        };
        assert_eq!(
            upgrade(Some("http://127.0.0.1:7700"), PROTOCOL),
            101,
            "this host's page"
        );
        assert_eq!(upgrade(None, PROTOCOL), 101, "a client without an origin");
        assert_eq!(upgrade(Some("http://evil.example"), PROTOCOL), 403);
        assert_eq!(upgrade(None, "apz.events.v1"), 400);
    }

    #[test]
    fn a_text_frame_that_is_not_a_ping_is_answered_and_the_view_stays_open() {
        let (_tx, rx) = mpsc::channel::<Event>();
        let mut socket = Script::default();
        socket
            .inbound
            .push_back(Message::Text(r#"{"t":"zoom"}"#.into()));
        watch(&rx, 9, 5, &mut socket);
        assert!(socket.texts().iter().any(|t| t["t"] == "error"));
        assert_eq!(
            socket.closed, None,
            "the script ran out (the socket is gone) without a close from the hub"
        );
    }
}
