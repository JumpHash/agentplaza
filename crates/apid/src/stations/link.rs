//! One workstation's live link: the socket its daemon dialed, the requests the hub sends on it and the
//! answers it waits for, and the events the daemon sends unasked (fanned out to subscribers).
//!
//! The wire rules are the ones `apz_station::link` documents: hello first, one answer per request id,
//! `Event` frames any time, and the hub's notices. The hub keeps the link alive with a `Facts` request every
//! [`HEARTBEAT`]; any answer counts as a sign of life, and a station silent for 30 s is offline
//! (`registry::OFFLINE_AFTER_MS`). A new link for a station replaces the old one, which is ended.

use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use apz_http::ws::{Message, Socket};
use apz_station::{Event, Facts, HubFrame, Notice, Op, StationFrame};
use serde_json::Value;

use crate::events::QUEUE;

/// How long one read of the socket waits before the loop looks at its outgoing frames again.
pub const POLL: Duration = Duration::from_millis(50);
/// How long a new link may take to say hello.
pub const HELLO_WITHIN: Duration = Duration::from_secs(10);
/// How often the hub asks a station for its facts, as its heartbeat.
pub const HEARTBEAT: Duration = Duration::from_secs(10);
/// The error a request ends with when its link ends first.
pub const LINK_CLOSED: &str = "the workstation's link closed";

/// What the loop writes on the socket.
pub enum Outgoing {
    Frame(HubFrame),
    Close { code: u16, reason: String },
}

/// The socket as the loop uses it: text in, text out. Binary frames are a protocol error.
pub trait Wire {
    fn put(&mut self, text: &str) -> io::Result<()>;
    /// The next text message, or `None` when nothing came within `wait`.
    fn get(&mut self, wait: Duration) -> io::Result<Option<String>>;
    fn end(&mut self, code: u16, reason: &str);
}

impl Wire for Socket {
    fn put(&mut self, text: &str) -> io::Result<()> {
        self.send(&Message::Text(text.to_string()))
    }

    fn get(&mut self, wait: Duration) -> io::Result<Option<String>> {
        match self.recv(wait)? {
            None => Ok(None),
            Some(Message::Text(text)) => Ok(Some(text)),
            Some(Message::Binary(_)) => {
                self.close(1003, "text messages only");
                Err(io::Error::new(io::ErrorKind::InvalidData, "a binary frame"))
            }
        }
    }

    fn end(&mut self, code: u16, reason: &str) {
        self.close(code, reason);
    }
}

/// What the hub does with the daemon's hello and its events. The loop calls these on its own thread, so
/// an implementation must not wait on this link's answers: it spawns that work instead.
pub trait Hooks {
    /// The daemon's hello. `Err` refuses the link: the socket closes with the reason.
    fn hello(&self, link: &Arc<StationLink>, version: &str, facts: Facts) -> Result<(), String>;
    fn event(&self, link: &Arc<StationLink>, event: Event);
}

/// The hub's handle on one station's link. Shared by the registry, the routes and the link's own loop.
pub struct StationLink {
    station: String,
    id: u64,
    outgoing: Sender<Outgoing>,
    /// The answers awaited, by request id. A request that times out or whose link ends is removed.
    pending: Mutex<BTreeMap<u64, SyncSender<Result<Value, String>>>>,
    next: AtomicU64,
    /// When the link last spoke, in ms.
    last_rx: AtomicU64,
    closed: AtomicBool,
    draining: AtomicBool,
    subscribers: Mutex<Vec<SyncSender<Event>>>,
}

impl StationLink {
    /// A link for `station`, generation `id`, that has just spoken at `now`; with the receiver the loop reads.
    pub fn new(station: &str, id: u64, now: u64) -> (Arc<StationLink>, Receiver<Outgoing>) {
        let (outgoing, rx) = mpsc::channel();
        let link = StationLink {
            station: station.to_string(),
            id,
            outgoing,
            pending: Mutex::new(BTreeMap::new()),
            next: AtomicU64::new(0),
            last_rx: AtomicU64::new(now),
            closed: AtomicBool::new(false),
            draining: AtomicBool::new(false),
            subscribers: Mutex::new(Vec::new()),
        };
        (Arc::new(link), rx)
    }

    pub fn station(&self) -> &str {
        &self.station
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn last_seen_ms(&self) -> u64 {
        self.last_rx.load(Ordering::Relaxed)
    }

    pub fn touch(&self, now: u64) {
        self.last_rx.store(now, Ordering::Relaxed);
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }

    /// Draining: no requests but freezes and settings go out, and the link closes once its answers are in.
    pub fn set_draining(&self, on: bool) {
        self.draining.store(on, Ordering::SeqCst);
    }

    /// A receiver of this link's events (frames, sync changes, owner input, logs, facts, session ends). It
    /// ends when the link does, and a subscriber that falls [`QUEUE`] events behind is dropped. The live view
    /// (the phase after this one) is the first production subscriber.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn subscribe(&self) -> Receiver<Event> {
        let (tx, rx) = mpsc::sync_channel(QUEUE);
        lock(&self.subscribers).push(tx);
        rx
    }

    /// Fans an event out to the subscribers.
    pub fn publish(&self, event: &Event) {
        lock(&self.subscribers).retain(|s| s.try_send(event.clone()).is_ok());
    }

    /// Sends `op` and waits up to `timeout` for its answer. Errors: the link is not up, a draining station
    /// refuses the op, the station answered `ok: false` (its error text), no answer came in time, or the
    /// link ended first.
    pub fn request(&self, op: Op, timeout: Duration) -> Result<Value, String> {
        if self.is_closed() {
            return Err("the workstation is not linked".into());
        }
        if self.is_draining() && !matches!(op, Op::Freeze { .. } | Op::Settings(_)) {
            return Err("the workstation is draining".into());
        }
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = mpsc::sync_channel(1);
        lock(&self.pending).insert(id, tx);
        // The link may have ended between the check above and the insert; `finish` sets `closed` before it
        // empties `pending`, so one of the two sees the other.
        if self.is_closed() {
            lock(&self.pending).remove(&id);
            return Err(LINK_CLOSED.into());
        }
        if self
            .outgoing
            .send(Outgoing::Frame(HubFrame::Request { id, op }))
            .is_err()
        {
            lock(&self.pending).remove(&id);
            return Err(LINK_CLOSED.into());
        }
        match rx.recv_timeout(timeout) {
            Ok(answer) => answer,
            Err(RecvTimeoutError::Timeout) => {
                lock(&self.pending).remove(&id);
                Err("the workstation did not answer in time".into())
            }
            Err(RecvTimeoutError::Disconnected) => Err(LINK_CLOSED.into()),
        }
    }

    /// Tells the daemon something the hub decided.
    pub fn notify(&self, notice: Notice) {
        let _ = self
            .outgoing
            .send(Outgoing::Frame(HubFrame::Notify { notice }));
    }

    /// Ends the link: the socket closes with `code` and `reason`.
    pub fn kill(&self, code: u16, reason: &str) {
        let _ = self.outgoing.send(Outgoing::Close {
            code,
            reason: reason.to_string(),
        });
    }

    /// Answers one awaited request (an answer nobody waits for is dropped).
    fn answer(&self, id: u64, ok: bool, result: Option<Value>, error: Option<String>) {
        if let Some(tx) = lock(&self.pending).remove(&id) {
            let answer = if ok {
                Ok(result.unwrap_or(Value::Null))
            } else {
                Err(error.unwrap_or_else(|| "the workstation refused the request".into()))
            };
            let _ = tx.send(answer);
        }
    }

    /// The hub's heartbeat: a facts request nobody waits for. Its answer still counts as a sign of life.
    fn beat(&self) {
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, _unread) = mpsc::sync_channel(1);
        lock(&self.pending).insert(id, tx);
        let _ = self
            .outgoing
            .send(Outgoing::Frame(HubFrame::Request { id, op: Op::Facts }));
    }

    fn pending_len(&self) -> usize {
        lock(&self.pending).len()
    }

    /// Marks the link ended, and fails every request still waiting on it.
    pub fn finish(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let waiting = std::mem::take(&mut *lock(&self.pending));
        for tx in waiting.into_values() {
            let _ = tx.send(Err(LINK_CLOSED.into()));
        }
        lock(&self.subscribers).clear();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Runs one link until it ends: writes what the hub queued, reads what the daemon sends, and heartbeats. Returns
/// `Ok` when the link ended by the hub's choice (a kill, a drain, a replacement) and `Err` when the daemon or the
/// socket broke. The caller finishes the link afterwards.
pub fn serve(
    wire: &mut dyn Wire,
    link: &Arc<StationLink>,
    outgoing: &Receiver<Outgoing>,
    hooks: &dyn Hooks,
) -> Result<(), String> {
    let started = Instant::now();
    let mut last_beat = Instant::now();
    let mut greeted = false;
    loop {
        loop {
            match outgoing.try_recv() {
                Ok(Outgoing::Frame(frame)) => {
                    let text = serde_json::to_string(&frame).map_err(|e| e.to_string())?;
                    wire.put(&text).map_err(|e| e.to_string())?;
                }
                Ok(Outgoing::Close { code, reason }) => {
                    wire.end(code, &reason);
                    return Ok(());
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        if greeted && link.is_draining() && link.pending_len() == 0 {
            wire.end(1000, "drained");
            return Ok(());
        }
        if !greeted && started.elapsed() > HELLO_WITHIN {
            wire.end(1002, "no hello");
            return Err("no hello within the limit".into());
        }
        if greeted && last_beat.elapsed() >= HEARTBEAT {
            link.beat();
            last_beat = Instant::now();
        }
        match wire.get(POLL) {
            Ok(None) => {}
            Ok(Some(text)) => {
                let frame: StationFrame = match serde_json::from_str(&text) {
                    Ok(frame) => frame,
                    Err(err) => {
                        wire.end(1002, "a frame the protocol does not know");
                        return Err(format!("a frame the protocol does not know: {err}"));
                    }
                };
                link.touch(apz_plaza::now_ms());
                match frame {
                    StationFrame::Hello { version, facts, .. } => {
                        if greeted {
                            wire.end(1002, "hello once per link");
                            return Err("a second hello".into());
                        }
                        if let Err(refusal) = hooks.hello(link, &version, facts) {
                            wire.end(1008, &refusal);
                            return Err(refusal);
                        }
                        greeted = true;
                    }
                    _ if !greeted => {
                        wire.end(1002, "hello first");
                        return Err("a frame before hello".into());
                    }
                    StationFrame::Response {
                        id,
                        ok,
                        result,
                        error,
                    } => link.answer(id, ok, result, error),
                    StationFrame::Event { event } => {
                        link.publish(&event);
                        hooks.event(link, event);
                    }
                }
            }
            Err(err) => return Err(err.to_string()),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use apz_station::{AccountMode, Form, Os, StationSettings};
    use std::thread;

    /// A daemon's end of the socket, in memory: the test sends what the daemon says, reads what the hub wrote,
    /// and sees how the link ended.
    struct Scripted {
        inbound: Receiver<String>,
        written: Sender<String>,
        ended: Sender<(u16, String)>,
    }

    impl Wire for Scripted {
        fn put(&mut self, text: &str) -> io::Result<()> {
            self.written
                .send(text.to_string())
                .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
        }

        fn get(&mut self, wait: Duration) -> io::Result<Option<String>> {
            match self.inbound.recv_timeout(wait) {
                Ok(text) => Ok(Some(text)),
                Err(RecvTimeoutError::Timeout) => Ok(None),
                Err(RecvTimeoutError::Disconnected) => {
                    Err(io::Error::from(io::ErrorKind::UnexpectedEof))
                }
            }
        }

        fn end(&mut self, code: u16, reason: &str) {
            let _ = self.ended.send((code, reason.to_string()));
        }
    }

    /// The daemon's side: the sender of its frames, the reader of the hub's, and how the link ended.
    pub(crate) struct Daemon {
        pub(crate) say: Sender<String>,
        pub(crate) hears: Receiver<String>,
        pub(crate) ended: Receiver<(u16, String)>,
    }

    impl Daemon {
        pub(crate) fn say(&self, frame: &StationFrame) {
            self.say
                .send(serde_json::to_string(frame).unwrap())
                .unwrap();
        }

        /// The next frame the hub wrote, parsed.
        pub(crate) fn hears(&self) -> HubFrame {
            let text = self
                .hears
                .recv_timeout(Duration::from_secs(5))
                .expect("the hub wrote a frame");
            serde_json::from_str(&text).expect("a hub frame")
        }
    }

    /// Serves one link on a thread with a `Scripted` wire, and gives the test its daemon.
    pub(crate) fn start(
        station: &str,
        hooks: Arc<dyn Hooks + Send + Sync>,
    ) -> (
        Arc<StationLink>,
        Daemon,
        thread::JoinHandle<Result<(), String>>,
    ) {
        let (link, outgoing) = StationLink::new(station, 1, apz_plaza::now_ms());
        let (say, inbound) = mpsc::channel();
        let (written, hears) = mpsc::channel();
        let (ended_tx, ended) = mpsc::channel();
        let mut wire = Scripted {
            inbound,
            written,
            ended: ended_tx,
        };
        let served = Arc::clone(&link);
        let handle = thread::spawn(move || {
            let result = serve(&mut wire, &served, &outgoing, hooks.as_ref());
            served.finish();
            result
        });
        (link, Daemon { say, hears, ended }, handle)
    }

    /// Accepts any hello, and records the facts it saw.
    pub(crate) struct Accept;

    impl Hooks for Accept {
        fn hello(
            &self,
            _link: &Arc<StationLink>,
            _version: &str,
            _facts: Facts,
        ) -> Result<(), String> {
            Ok(())
        }

        fn event(&self, _link: &Arc<StationLink>, _event: Event) {}
    }

    pub(crate) fn facts() -> Facts {
        Facts {
            os: Os::Linux,
            os_version: "6.8".into(),
            arch: "x86_64".into(),
            hostname: "desk-1".into(),
            form: Form::Desktop,
            cpus: 8,
            ram_mb: 16384,
            gpus: vec![],
            displays: vec![],
            account: AccountMode::AgentUser {
                name: "agentws".into(),
            },
            apps: 3,
            virtual_displays: true,
            ui_tree: true,
            machine_id: None,
        }
    }

    pub(crate) fn hello() -> StationFrame {
        StationFrame::Hello {
            station: "ws-7".into(),
            version: "0.0.8".into(),
            facts: facts(),
        }
    }

    /// Greets the link with a hello and answers the first facts request the test sends with it.
    pub(crate) fn greet(link: &Arc<StationLink>, daemon: &Daemon) {
        daemon.say(&hello());
        let asker = Arc::clone(link);
        let ask = thread::spawn(move || asker.request(Op::Facts, Duration::from_secs(5)));
        let HubFrame::Request { id, op } = daemon.hears() else {
            panic!("a request")
        };
        assert_eq!(op, Op::Facts);
        daemon.say(&StationFrame::Response {
            id,
            ok: true,
            result: Some(serde_json::to_value(facts()).unwrap()),
            error: None,
        });
        assert!(ask.join().unwrap().is_ok());
    }

    #[test]
    fn two_requests_in_flight_are_answered_out_of_order_by_id() {
        let (link, daemon, handle) = start("w1", Arc::new(Accept));
        greet(&link, &daemon);
        let (a, b) = (Arc::clone(&link), Arc::clone(&link));
        let first =
            thread::spawn(move || a.request(Op::Freeze { on: false }, Duration::from_secs(5)));
        let second = thread::spawn(move || {
            b.request(
                Op::Sync(apz_station::SyncOp::Status),
                Duration::from_secs(5),
            )
        });
        let mut ids = Vec::new();
        for _ in 0..2 {
            if let HubFrame::Request { id, op } = daemon.hears() {
                ids.push((id, op));
            }
        }
        for (id, op) in ids.iter().rev() {
            let result = if *op == (Op::Freeze { on: false }) {
                Value::Null
            } else {
                serde_json::json!({"files": 2})
            };
            daemon.say(&StationFrame::Response {
                id: *id,
                ok: true,
                result: Some(result),
                error: None,
            });
        }
        assert_eq!(first.join().unwrap(), Ok(Value::Null));
        assert_eq!(second.join().unwrap(), Ok(serde_json::json!({"files": 2})));
        link.kill(1000, "done");
        let _ = handle.join();
    }

    #[test]
    fn a_refusal_carries_the_stations_error() {
        let (link, daemon, handle) = start("w1", Arc::new(Accept));
        greet(&link, &daemon);
        let asker = Arc::clone(&link);
        let ask = thread::spawn(move || {
            asker.request(
                Op::Input {
                    session: 9,
                    action: apz_station::InputAction::Type { text: "x".into() },
                },
                Duration::from_secs(5),
            )
        });
        let HubFrame::Request { id, .. } = daemon.hears() else {
            panic!("a request")
        };
        daemon.say(&StationFrame::Response {
            id,
            ok: false,
            result: None,
            error: Some("frozen by STOP ALL".into()),
        });
        assert_eq!(ask.join().unwrap(), Err("frozen by STOP ALL".to_string()));
        link.kill(1000, "done");
        let _ = handle.join();
    }

    #[test]
    fn a_request_without_an_answer_times_out_and_a_late_answer_is_dropped() {
        let (link, daemon, handle) = start("w1", Arc::new(Accept));
        greet(&link, &daemon);
        let asker = Arc::clone(&link);
        let ask = thread::spawn(move || asker.request(Op::Facts, Duration::from_millis(100)));
        let HubFrame::Request { id, .. } = daemon.hears() else {
            panic!("a request")
        };
        assert_eq!(
            ask.join().unwrap(),
            Err("the workstation did not answer in time".to_string())
        );
        daemon.say(&StationFrame::Response {
            id,
            ok: true,
            result: None,
            error: None,
        });
        let asker = Arc::clone(&link);
        let again = thread::spawn(move || asker.request(Op::Facts, Duration::from_secs(5)));
        let HubFrame::Request { id: next, .. } = daemon.hears() else {
            panic!("a request")
        };
        assert!(next != id, "the next request has its own id");
        daemon.say(&StationFrame::Response {
            id: next,
            ok: true,
            result: Some(serde_json::json!(1)),
            error: None,
        });
        assert_eq!(
            again.join().unwrap(),
            Ok(serde_json::json!(1)),
            "the link still answers after a late answer"
        );
        link.kill(1000, "done");
        let _ = handle.join();
    }

    #[test]
    fn requests_in_flight_fail_when_the_daemon_drops_the_link() {
        let (link, daemon, handle) = start("w1", Arc::new(Accept));
        greet(&link, &daemon);
        let asker = Arc::clone(&link);
        let ask = thread::spawn(move || asker.request(Op::Facts, Duration::from_secs(30)));
        let _ = daemon.hears();
        drop(daemon);
        assert_eq!(ask.join().unwrap(), Err(LINK_CLOSED.to_string()));
        assert!(
            handle.join().unwrap().is_err(),
            "the broken link ends with an error"
        );
        assert!(link.is_closed());
        assert_eq!(
            link.request(Op::Facts, Duration::from_millis(10)),
            Err("the workstation is not linked".to_string())
        );
    }

    #[test]
    fn a_frame_before_hello_closes_the_link() {
        let (_link, daemon, handle) = start("w1", Arc::new(Accept));
        daemon.say(&StationFrame::Response {
            id: 1,
            ok: true,
            result: None,
            error: None,
        });
        assert_eq!(daemon.ended.recv().unwrap().0, 1002);
        assert!(handle.join().unwrap().is_err());
    }

    #[test]
    fn a_hello_the_hub_refuses_closes_the_link_with_the_reason() {
        struct Refuse;
        impl Hooks for Refuse {
            fn hello(&self, _: &Arc<StationLink>, _: &str, _: Facts) -> Result<(), String> {
                Err("this machine is the hub's own host".into())
            }
            fn event(&self, _: &Arc<StationLink>, _: Event) {}
        }
        let (_link, daemon, handle) = start("w1", Arc::new(Refuse));
        daemon.say(&hello());
        assert_eq!(
            daemon.ended.recv().unwrap(),
            (1008, "this machine is the hub's own host".to_string())
        );
        assert!(handle.join().unwrap().is_err());
    }

    #[test]
    fn events_reach_the_subscribers_and_the_hooks() {
        struct Record(Sender<String>);
        impl Hooks for Record {
            fn hello(&self, _: &Arc<StationLink>, _: &str, _: Facts) -> Result<(), String> {
                Ok(())
            }
            fn event(&self, _: &Arc<StationLink>, event: Event) {
                let _ = self.0.send(format!("{event:?}"));
            }
        }
        let (seen_tx, seen) = mpsc::channel();
        let (link, daemon, handle) = start("w1", Arc::new(Record(seen_tx)));
        let subscriber = link.subscribe();
        greet(&link, &daemon);
        daemon.say(&StationFrame::Event {
            event: Event::OwnerInput { session: 9 },
        });
        let got = subscriber
            .recv_timeout(Duration::from_secs(5))
            .expect("the subscriber gets the event");
        assert_eq!(got, Event::OwnerInput { session: 9 });
        assert!(
            seen.recv_timeout(Duration::from_secs(5))
                .unwrap()
                .contains("OwnerInput")
        );
        link.kill(1000, "done");
        let _ = handle.join();
        assert!(
            subscriber.recv_timeout(Duration::from_secs(1)).is_err(),
            "a subscriber ends with its link"
        );
    }

    #[test]
    fn a_drained_link_closes_once_its_answers_are_in() {
        let (link, daemon, handle) = start("w1", Arc::new(Accept));
        greet(&link, &daemon);
        let asker = Arc::clone(&link);
        let ask = thread::spawn(move || asker.request(Op::Facts, Duration::from_secs(5)));
        let HubFrame::Request { id, .. } = daemon.hears() else {
            panic!("a request")
        };
        link.set_draining(true);
        assert!(
            link.request(
                Op::Run(apz_station::RunOp::Stop {
                    run: "run-1".into()
                }),
                Duration::from_millis(50)
            )
            .is_err(),
            "no new work while draining"
        );
        daemon.say(&StationFrame::Response {
            id,
            ok: true,
            result: Some(Value::Null),
            error: None,
        });
        assert!(ask.join().unwrap().is_ok());
        assert_eq!(daemon.ended.recv().unwrap(), (1000, "drained".to_string()));
        assert!(handle.join().unwrap().is_ok());
    }

    #[test]
    fn the_settings_go_out_as_one_request_and_a_freeze_on_a_draining_link_still_goes() {
        let (link, daemon, handle) = start("w1", Arc::new(Accept));
        greet(&link, &daemon);
        let settings = StationSettings {
            record: false,
            ..StationSettings::default()
        };
        let pusher = Arc::clone(&link);
        let push = thread::spawn(move || {
            pusher.request(Op::Settings(settings.clone()), Duration::from_secs(5))
        });
        let HubFrame::Request { id, op } = daemon.hears() else {
            panic!("a request")
        };
        assert!(matches!(op, Op::Settings(s) if !s.record));
        daemon.say(&StationFrame::Response {
            id,
            ok: true,
            result: None,
            error: None,
        });
        assert!(push.join().unwrap().is_ok());
        // A draining link closes once nothing is in flight, so one request stays open while the freeze
        // goes out (without it the link could close before the freeze was sent).
        let asker = Arc::clone(&link);
        let ask = thread::spawn(move || asker.request(Op::Facts, Duration::from_secs(5)));
        let HubFrame::Request { id: held, .. } = daemon.hears() else {
            panic!("a request")
        };
        link.set_draining(true);
        let freezer = Arc::clone(&link);
        let freeze =
            thread::spawn(move || freezer.request(Op::Freeze { on: true }, Duration::from_secs(5)));
        let HubFrame::Request { id, op } = daemon.hears() else {
            panic!("a request")
        };
        assert_eq!(op, Op::Freeze { on: true });
        daemon.say(&StationFrame::Response {
            id,
            ok: true,
            result: None,
            error: None,
        });
        assert!(freeze.join().unwrap().is_ok());
        daemon.say(&StationFrame::Response {
            id: held,
            ok: true,
            result: Some(Value::Null),
            error: None,
        });
        assert!(ask.join().unwrap().is_ok());
        assert_eq!(daemon.ended.recv().unwrap(), (1000, "drained".to_string()));
        assert!(handle.join().unwrap().is_ok());
    }
}
