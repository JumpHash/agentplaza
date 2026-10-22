//! The event stream: `GET /v1/ws`, a WebSocket fed by every module and by relays of the plaza's
//! domain events and noded's VM events. One text message per event, `{"event": <kind>, "seq": <n>, "data":
//! <data>}`. `seq` grows by one per event published; a socket opens with a `hello` event naming the hub's boot, its
//! version and the schema of its store and the `seq` it starts after (PLAN 30.4): a console that sees a gap in `seq`,
//! a new boot, version or schema revalidates every list it shows (with `If-None-Match`), because events are hints
//! and the bus keeps no backlog. The socket is read-only and bounded: a client that falls [`QUEUE`] events behind is
//! closed (1013, too slow), and the caps are [`PER_PEER`] sockets per peer address and [`TOTAL`]
//! in all, refused with 429 before the upgrade.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use apz_http::client::Target;
use apz_http::ws::{self, Limits, Message, Origins, Socket};
use apz_http::{Response, RouteDoc, Router};
use apz_metrics::{Counter, EntityKind, Gauge, Hist, SeriesKey};
use serde::{Deserialize, Serialize};

use crate::state::App;

/// The entity of the hub-wide `hub.*` series.
const HUB_ENTITY: &str = "hub";

/// The subprotocol a console asks for.
const PROTOCOL: &str = "apz.events.v1";
/// The subprotocol noded speaks.
const NODED_PROTOCOL: &str = "apz.noded.v1";
/// Events a subscriber may hold unsent before it is too slow.
pub const QUEUE: usize = 1024;
/// Sockets one peer address may hold open.
pub const PER_PEER: usize = 8;
/// Sockets the hub holds open in all.
pub const TOTAL: usize = 64;
/// How long a socket waits for a client message between batches of events.
const POLL: Duration = Duration::from_millis(250);
/// How long the relay waits for noded to answer the handshake.
const CONNECT: Duration = Duration::from_secs(10);

/// One event on the bus: its kind, its one-line JSON data, and when it was published (a socket
/// measures its queue wait from this).
#[derive(Debug, Clone)]
pub struct Frame {
    pub kind: String,
    pub data: String,
    pub queued: Instant,
    /// The bus's sequence number of this event (1, 2, …; gaps tell a console it missed events).
    pub seq: u64,
}

impl Frame {
    /// The socket's text message: `{"event": <kind>, "seq": <n>, "data": <data>}`. Data that is not JSON goes
    /// out as `null`, so a message is always one valid JSON object.
    pub fn wire(&self) -> String {
        let data = serde_json::from_str::<serde_json::Value>(&self.data)
            .unwrap_or(serde_json::Value::Null);
        let kind = serde_json::to_string(&self.kind).unwrap_or_else(|_| "\"\"".into());
        format!("{{\"event\":{kind},\"seq\":{},\"data\":{data}}}", self.seq)
    }
}

/// Fan-out to every live subscriber; no backlog. A subscriber whose queue is full is dropped: its
/// socket then closes as too slow, after the events it already holds.
#[derive(Default)]
pub struct Bus {
    subscribers: Mutex<Vec<SyncSender<Frame>>>,
    instruments: OnceLock<Instruments>,
    /// The last sequence number given out.
    seq: AtomicU64,
    /// This boot of the hub (a random id): a console that sees another reloads what it shows.
    boot: OnceLock<String>,
}

/// The bus's own series, handed over by [`instrument_bus`].
struct Instruments {
    events: Counter,
    clients: Gauge,
    lag: Hist,
}

impl Bus {
    /// Sets the handles the bus records into: `hub.events` for each publish, `hub.ws_clients` for the
    /// subscriber count, and `hub.bus_lag_ms` for how long an event waits before its socket writes it.
    /// Only the first call takes effect.
    pub fn instrument(&self, events: Counter, clients: Gauge, lag: Hist) {
        let _ = self.instruments.set(Instruments {
            events,
            clients,
            lag,
        });
    }

    /// The lag histogram, once [`Bus::instrument`] has run.
    fn lag(&self) -> Option<Hist> {
        self.instruments.get().map(|i| i.lag.clone())
    }

    pub fn publish(&self, kind: &str, data: &impl Serialize) {
        let data = serde_json::to_string(data).unwrap_or_else(|_| "null".into());
        self.publish_raw(kind, data);
    }

    /// `data` must already be one line of JSON.
    pub fn publish_raw(&self, kind: &str, data: String) {
        if let Some(instruments) = self.instruments.get() {
            instruments.events.inc();
        }
        let mut subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        // Numbered under the subscribers' lock, so every subscriber receives events in their seq order.
        let frame = Frame {
            kind: kind.to_string(),
            data,
            queued: Instant::now(),
            seq: self.seq.fetch_add(1, Ordering::AcqRel) + 1,
        };
        subscribers.retain(|s| s.try_send(frame.clone()).is_ok());
        if let Some(instruments) = self.instruments.get() {
            instruments.clients.set(subscribers.len() as f64);
        }
    }

    /// A subscription with a queue of [`QUEUE`] events.
    pub fn subscribe(&self) -> Receiver<Frame> {
        self.subscribe_from().0
    }

    /// A subscription, and the sequence number of the last event published before it: the first event it receives
    /// is the next one.
    pub fn subscribe_from(&self) -> (Receiver<Frame>, u64) {
        let (tx, rx) = std::sync::mpsc::sync_channel(QUEUE);
        let mut subscribers = self
            .subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        subscribers.push(tx);
        let seq = self.seq.load(Ordering::Acquire);
        if let Some(instruments) = self.instruments.get() {
            instruments.clients.set(subscribers.len() as f64);
        }
        (rx, seq)
    }

    /// This boot's id: random, fixed for the life of the process.
    pub fn boot(&self) -> &str {
        self.boot.get_or_init(|| {
            let mut bytes = [0u8; 8];
            // Zeros only if the OS gives no randomness: a boot id still, just not a distinct one.
            let _ = apz_os::random::fill(&mut bytes);
            bytes.iter().map(|b| format!("{b:02x}")).collect()
        })
    }
}

/// Gives the bus its `hub.*` series from the hub's registry. Called once at startup, from
/// `metrics::collect::spawn`.
pub fn instrument_bus(app: &App) {
    let registry = &app.metrics.registry;
    app.bus.instrument(
        registry.counter(SeriesKey::new("hub.events", EntityKind::Hub, HUB_ENTITY)),
        registry.gauge(SeriesKey::new(
            "hub.ws_clients",
            EntityKind::Hub,
            HUB_ENTITY,
        )),
        registry.hist(SeriesKey::new(
            "hub.bus_lag_ms",
            EntityKind::Hub,
            HUB_ENTITY,
        )),
    );
}

/// The sockets open now, per peer and in all. A [`Slot`] holds one place until it drops.
pub struct Slots {
    open: Mutex<Open>,
}

struct Open {
    total: usize,
    per_peer: BTreeMap<Option<IpAddr>, usize>,
}

impl Slots {
    pub const fn new() -> Self {
        Slots {
            open: Mutex::new(Open {
                total: 0,
                per_peer: BTreeMap::new(),
            }),
        }
    }

    /// A place for a socket from `peer`, or `None` when that peer or the hub is at its cap.
    pub fn take(&'static self, peer: Option<IpAddr>) -> Option<Slot> {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        let held = open.per_peer.get(&peer).copied().unwrap_or(0);
        if held >= PER_PEER || open.total >= TOTAL {
            return None;
        }
        open.total += 1;
        *open.per_peer.entry(peer).or_insert(0) += 1;
        Some(Slot { slots: self, peer })
    }

    /// How many sockets are open now.
    #[cfg(test)]
    pub fn open_count(&self) -> usize {
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .total
    }

    fn release(&self, peer: Option<IpAddr>) {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        open.total = open.total.saturating_sub(1);
        let now = match open.per_peer.get_mut(&peer) {
            Some(held) => {
                *held = held.saturating_sub(1);
                *held
            }
            None => 0,
        };
        if now == 0 {
            open.per_peer.remove(&peer);
        }
    }
}

/// One socket's place in the caps; it frees the place when it drops.
pub struct Slot {
    slots: &'static Slots,
    peer: Option<IpAddr>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.slots.release(self.peer);
    }
}

/// The hub's sockets.
static SOCKETS: Slots = Slots::new();

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/ws", |app, req, _| {
            let Some(slot) = SOCKETS.take(req.peer) else {
                return Response::error(429, "too many open event sockets; close one first");
            };
            let (events, after) = app.bus.subscribe_from();
            let lag = app.bus.lag();
            let hello = Frame {
                kind: "hello".to_string(),
                data: serde_json::json!({
                    "boot": app.bus.boot(),
                    "version": apz_os::version(),
                    "schema": app.store.db(crate::store::PLAZA).schema(),
                    "seq": after,
                })
                .to_string(),
                queued: Instant::now(),
                seq: after,
            };
            ws::accept(
                req,
                Origins::SameHost,
                Some(PROTOCOL),
                Limits::default(),
                move |mut socket| {
                    let _slot = slot;
                    if socket.send(&Message::Text(hello.wire())).is_err() {
                        return;
                    }
                    serve(socket, events, lag);
                },
            )
        })
        .doc(RouteDoc::new("Open a socket that streams the hub's events").errors(&[429]))
}

/// Forwards the bus's events to `socket` until it ends: a client message closes it (read-only,
/// 1008), a queue that overflowed closes it (too slow, 1013), and a dead connection just ends. Each
/// event's wait since it was published goes into `lag` when one is given.
fn serve(mut socket: Socket, events: Receiver<Frame>, lag: Option<Hist>) {
    loop {
        loop {
            match events.try_recv() {
                Ok(frame) => {
                    if let Some(lag) = &lag {
                        lag.record(frame.queued.elapsed().as_secs_f64() * 1000.0);
                    }
                    if socket.send(&Message::Text(frame.wire())).is_err() {
                        return;
                    }
                }
                Err(TryRecvError::Empty) => break,
                // Only the bus drops a subscriber, and it does so when the queue is full.
                Err(TryRecvError::Disconnected) => {
                    socket.close(1013, "too slow; reconnect");
                    return;
                }
            }
        }
        match socket.recv(POLL) {
            Ok(None) => {}
            Ok(Some(_)) => {
                socket.close(1008, "this socket is read-only");
                return;
            }
            Err(_) => return,
        }
    }
}

/// Starts the relays: the plaza's domain events, and noded's `/v1/ws` (reconnecting with backoff
/// while noded is down).
pub fn spawn_relays(app: Arc<App>) {
    let plaza = app.plaza.events.subscribe();
    let relay = Arc::clone(&app);
    std::thread::spawn(move || {
        for event in plaza {
            relay.bus.publish_raw(event.kind(), event.data());
        }
    });
    std::thread::spawn(move || {
        let target = Target {
            base: format!("http://127.0.0.1:{}", app.noded.port),
            headers: app
                .noded
                .token
                .header()
                .map(|(name, value)| (name.to_string(), value))
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let mut backoff = Duration::from_secs(1);
        loop {
            if let Ok(mut socket) = ws::connect(
                &target,
                "/v1/ws",
                Some(NODED_PROTOCOL),
                CONNECT,
                Limits::default(),
            ) {
                backoff = Duration::from_secs(1);
                relay_noded(&app.bus, &mut socket);
            }
            std::thread::sleep(backoff);
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    });
}

/// Republishes noded's `{"event", "data"}` text messages on the bus until the socket ends. A
/// message that is not one, or a binary one, is not an event: a binary one closes the socket.
fn relay_noded(bus: &Bus, socket: &mut Socket) {
    #[derive(Deserialize)]
    struct Wire {
        event: String,
        data: serde_json::Value,
    }
    loop {
        match socket.recv(POLL) {
            Ok(Some(Message::Text(text))) => {
                if let Ok(wire) = serde_json::from_str::<Wire>(&text) {
                    bus.publish_raw(&wire.event, wire.data.to_string());
                }
            }
            Ok(Some(Message::Binary(_))) => {
                socket.close(1003, "text messages only");
                return;
            }
            Ok(None) => {}
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaked() -> &'static Slots {
        Box::leak(Box::new(Slots::new()))
    }

    #[test]
    fn the_ninth_socket_from_one_peer_is_refused() {
        let slots = leaked();
        let peer: Option<IpAddr> = Some("10.0.0.5".parse().unwrap());
        let held: Vec<Slot> = (0..PER_PEER)
            .map(|_| slots.take(peer).expect("under the cap"))
            .collect();
        assert!(
            slots.take(peer).is_none(),
            "a ninth socket from one peer is refused"
        );
        assert!(
            slots.take(Some("10.0.0.6".parse().unwrap())).is_some(),
            "another peer still gets one"
        );
        drop(held);
        assert!(
            slots.take(peer).is_some(),
            "a closed socket frees its place"
        );
    }

    #[test]
    fn the_hub_holds_sixty_four_sockets_in_all() {
        let slots = leaked();
        let mut held = Vec::new();
        for n in 0..(TOTAL / PER_PEER) {
            let ip = IpAddr::from([10, 0, 0, n as u8]);
            for _ in 0..PER_PEER {
                held.push(slots.take(Some(ip)).expect("under the cap"));
            }
        }
        assert_eq!(slots.open_count(), TOTAL);
        assert!(
            slots.take(Some(IpAddr::from([10, 0, 1, 1]))).is_none(),
            "a 65th socket is refused"
        );
    }

    #[test]
    fn a_bus_event_is_one_text_message_with_its_kind_and_data() {
        let bus = Bus::default();
        let rx = bus.subscribe();
        let event = apz_plaza::events::Event::Run {
            id: "r1".to_string(),
            state: "running",
        };
        bus.publish_raw(event.kind(), event.data());
        let frame = rx.try_recv().expect("the subscriber got the event");
        assert_eq!(
            frame.wire(),
            r#"{"event":"run","seq":1,"data":{"id":"r1","state":"running"}}"#
        );
        bus.publish_raw("run", "{}".to_string());
        assert_eq!(rx.try_recv().unwrap().seq, 2, "each event gets the next number");
        let (_late, after) = bus.subscribe_from();
        assert_eq!(after, 2, "a subscriber learns the seq it starts after");
    }

    #[test]
    fn a_publish_counts_into_hub_events_and_sets_the_client_count() {
        let registry = apz_metrics::Registry::new(64);
        let events = registry.counter(SeriesKey::new("hub.events", EntityKind::Hub, HUB_ENTITY));
        let clients = registry.gauge(SeriesKey::new(
            "hub.ws_clients",
            EntityKind::Hub,
            HUB_ENTITY,
        ));
        let lag = registry.hist(SeriesKey::new(
            "hub.bus_lag_ms",
            EntityKind::Hub,
            HUB_ENTITY,
        ));
        let bus = Bus::default();
        bus.instrument(events.clone(), clients.clone(), lag);
        let _first = bus.subscribe();
        let _second = bus.subscribe();
        assert_eq!(clients.get(), 2.0, "each subscription counts as a client");
        bus.publish_raw("run", "null".to_string());
        bus.publish_raw("run", "null".to_string());
        assert_eq!(events.total(), 2.0, "each publish counts once");
    }

    #[test]
    fn a_subscriber_a_queue_behind_is_dropped_after_what_it_holds() {
        let bus = Bus::default();
        let rx = bus.subscribe();
        for _ in 0..=QUEUE {
            bus.publish_raw("run", "null".to_string());
        }
        assert_eq!(rx.try_iter().count(), QUEUE);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Disconnected)));
    }
}

