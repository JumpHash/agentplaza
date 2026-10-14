//! The outgoing side of the link, shared by every thread that answers a request or emits an event.
//!
//! Each connection gets a new generation when it attaches. A response is sent only when the connection its
//! request arrived on is still the current one, so an answer never reaches a later link with the same id.
//! Events go to whichever connection is current, and are dropped while the daemon is between links.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use apz_station::{Event, StationFrame};
use serde_json::Value;

/// The outgoing queue of the link, cloneable across threads.
#[derive(Clone, Default)]
pub struct Uplink {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    generation: u64,
    tx: Option<Sender<StationFrame>>,
}

impl Uplink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attaches a connection's queue and returns its generation.
    pub fn attach(&self, tx: Sender<StationFrame>) -> u64 {
        let mut inner = self.lock();
        inner.generation += 1;
        inner.tx = Some(tx);
        inner.generation
    }

    /// Detaches the current connection; anything answered for it from now on is dropped.
    pub fn detach(&self) {
        let mut inner = self.lock();
        inner.generation += 1;
        inner.tx = None;
    }

    /// The current connection's generation.
    pub fn generation(&self) -> u64 {
        self.lock().generation
    }

    /// Sends `frame` on the current connection, if one is attached.
    pub fn send(&self, frame: StationFrame) {
        if let Some(tx) = &self.lock().tx {
            let _ = tx.send(frame);
        }
    }

    /// Sends an unsolicited event on the current connection.
    pub fn event(&self, event: Event) {
        self.send(StationFrame::Event { event });
    }

    /// Answers request `id`, which arrived on connection `generation`, if that connection is still current.
    pub fn respond(&self, generation: u64, id: u64, outcome: Result<Option<Value>, String>) {
        let inner = self.lock();
        if inner.generation != generation {
            return;
        }
        let Some(tx) = &inner.tx else { return };
        let frame = match outcome {
            Ok(result) => StationFrame::Response {
                id,
                ok: true,
                result,
                error: None,
            },
            Err(error) => StationFrame::Response {
                id,
                ok: false,
                result: None,
                error: Some(error),
            },
        };
        let _ = tx.send(frame);
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;

    use super::*;

    #[test]
    fn a_response_goes_only_to_the_connection_it_came_on() {
        let uplink = Uplink::new();
        let (first_tx, first_rx) = channel();
        let first = uplink.attach(first_tx);
        uplink.respond(first, 7, Ok(None));
        assert!(matches!(
            first_rx.try_recv(),
            Ok(StationFrame::Response {
                id: 7,
                ok: true,
                ..
            })
        ));

        let (second_tx, second_rx) = channel();
        uplink.attach(second_tx);
        uplink.respond(first, 8, Err("stale".into()));
        assert!(second_rx.try_recv().is_err(), "a stale answer is dropped");
    }

    #[test]
    fn events_are_dropped_between_links() {
        let uplink = Uplink::new();
        uplink.event(Event::OwnerInput { session: 1 });
        let (tx, rx) = channel();
        uplink.attach(tx);
        uplink.event(Event::OwnerInput { session: 2 });
        assert!(matches!(
            rx.try_recv(),
            Ok(StationFrame::Event {
                event: Event::OwnerInput { session: 2 }
            })
        ));
        uplink.detach();
        uplink.event(Event::OwnerInput { session: 3 });
        assert!(rx.try_recv().is_err());
    }
}
