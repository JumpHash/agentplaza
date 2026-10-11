//! The event stream: `GET /v1/events`, fed by every module and by relays of
//! the plaza's domain events and noded's VM events.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use apz_http::{Response, Router};
use serde::Serialize;

use crate::state::App;

/// One SSE frame: `event: <kind>` + one-line JSON `data`.
#[derive(Debug, Clone)]
pub struct Frame {
    pub kind: String,
    pub data: String,
}

/// Fan-out to every live subscriber; no backlog.
#[derive(Default)]
pub struct Bus {
    subscribers: Mutex<Vec<Sender<Frame>>>,
}

impl Bus {
    pub fn publish(&self, kind: &str, data: &impl Serialize) {
        let data = serde_json::to_string(data).unwrap_or_else(|_| "null".into());
        self.publish_raw(kind, data);
    }

    /// `data` must already be one line of JSON.
    pub fn publish_raw(&self, kind: &str, data: String) {
        let frame = Frame { kind: kind.to_string(), data };
        self.subscribers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|s| s.send(frame.clone()).is_ok());
    }

    pub fn subscribe(&self) -> Receiver<Frame> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.subscribers.lock().unwrap_or_else(PoisonError::into_inner).push(tx);
        rx
    }
}

const HEARTBEAT: Duration = Duration::from_secs(15);

pub fn routes(router: Router<App>) -> Router<App> {
    router.get("/v1/events", |app, _, _| {
        let rx = app.bus.subscribe();
        Response::sse(move |sse| loop {
            match rx.recv_timeout(HEARTBEAT) {
                Ok(frame) => sse.event(&frame.kind, &frame.data)?,
                Err(RecvTimeoutError::Timeout) => sse.heartbeat()?,
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
        })
    })
}

/// Starts the relays: the plaza's domain events, and noded's `/v1/events`
/// (reconnecting with backoff while noded is down).
pub fn spawn_relays(app: Arc<App>) {
    let plaza = app.plaza.events.subscribe();
    let relay = Arc::clone(&app);
    std::thread::spawn(move || {
        for event in plaza {
            relay.bus.publish_raw(event.kind(), event.data());
        }
    });
    std::thread::spawn(move || {
        let mut backoff = Duration::from_secs(1);
        loop {
            if let Ok(mut stream) = apz_http::client::events(app.noded.port, "/v1/events", HEARTBEAT * 2) {
                backoff = Duration::from_secs(1);
                while let Ok(Some(event)) = stream.next_event() {
                    app.bus.publish_raw(&event.event, event.data);
                }
            }
            std::thread::sleep(backoff);
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    });
}
