// Copyright 2026 AgentPlaza
// SPDX-License-Identifier: Apache-2.0
//
// Written for this port (PLAN 22.6, 17.4, round 2). A lab segment that spans nodes is one switch per node,
// and each node joins its switch to the others through a trunk: a mux stream opened as `kind: trunk`. The
// hub pairs the two nodes' trunk streams for a segment and splices their bytes, so the frames cross the
// node tunnel and no node ever dials another. The splice carries bytes only; the frames it carries are the
// segment's own, framed by the overlay.

//! Trunk streams on the node tunnel: the hub's pairing and splice, and the byte view a node's overlay reads and
//! writes.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use crate::mux::{Event, MAX_CHUNK, Stream};

/// How long a splice waits for an event before it looks again (a splice ends on the streams' own events).
const POLL: Duration = Duration::from_millis(50);

/// The kind a trunk stream's header names.
pub const KIND: &str = "trunk";

/// The segment a trunk stream opened with (its header names the kind and the segment), or `None` for any other
/// stream. A trunk that names no segment is `None` too: it is then answered as the unknown stream it is.
pub fn trunk_segment(stream: &Stream) -> Option<String> {
    let header: serde_json::Value = serde_json::from_slice(stream.header()).ok()?;
    if header["kind"].as_str() != Some(KIND) {
        return None;
    }
    header["segment"].as_str().map(str::to_owned)
}

/// Copies one direction of a splice: the events of `from` go to `to` until either end finishes, resets or closes.
fn pump(from: Arc<Stream>, to: Arc<Stream>) {
    loop {
        match from.recv(POLL) {
            Some(Event::Data(bytes)) => {
                if to.send(&bytes).is_err() {
                    from.reset("the other end of the trunk is gone");
                    return;
                }
            }
            Some(Event::Fin) => {
                to.finish().ok();
                return;
            }
            Some(Event::Reset(reason)) => {
                to.reset(&reason);
                return;
            }
            Some(Event::Closed) => {
                to.reset("the trunk's node is gone");
                return;
            }
            None => {}
        }
    }
}

/// Splices two trunk streams: each one's bytes go to the other, in both directions, until they end.
pub fn splice(a: Stream, b: Stream) {
    let (a, b) = (Arc::new(a), Arc::new(b));
    let (a_to_b, b_to_a) = (
        thread::Builder::new().name("trunk-splice".into()),
        thread::Builder::new().name("trunk-splice".into()),
    );
    let (fa, tb) = (Arc::clone(&a), Arc::clone(&b));
    a_to_b.spawn(move || pump(fa, tb)).ok();
    b_to_a.spawn(move || pump(b, a)).ok();
}

/// The hub's pairing of trunks: a segment's first trunk waits, and its second is spliced to it.
#[derive(Default)]
pub struct TrunkHub {
    waiting: Mutex<BTreeMap<String, Arc<Stream>>>,
}

impl TrunkHub {
    /// A hub with no trunk waiting.
    pub fn new() -> Self {
        Self::default()
    }

    /// Offers `stream` as the trunk of `key` (a segment's name, scoped by the topology). The first one waits for
    /// its pair; the second is spliced to the first.
    pub fn offer(&self, key: &str, stream: Stream) {
        let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        match waiting.remove(key) {
            Some(first) => {
                drop(waiting);
                let first = Arc::try_unwrap(first).ok();
                match first {
                    Some(first) => splice(first, stream),
                    None => stream.reset("the segment's first trunk is still in use"),
                }
            }
            None => {
                waiting.insert(key.to_owned(), Arc::new(stream));
            }
        }
    }

    /// How many trunks wait for their pair.
    pub fn waiting(&self) -> usize {
        self.waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

/// The reading half of a trunk, as bytes: what the other side sent, and the end of it when that side ends.
pub struct TrunkReader {
    stream: Arc<Stream>,
    pending: Vec<u8>,
}

/// The writing half of a trunk: its bytes go to the other side. Dropping it ends this side's sending.
pub struct TrunkWriter {
    stream: Arc<Stream>,
}

/// Splits a trunk stream into its byte halves, for an overlay that reads and writes separately.
pub fn halves(stream: Stream) -> (TrunkReader, TrunkWriter) {
    let stream = Arc::new(stream);
    (
        TrunkReader {
            stream: Arc::clone(&stream),
            pending: Vec::new(),
        },
        TrunkWriter { stream },
    )
}

impl Read for TrunkReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pending.is_empty() {
            loop {
                match self.stream.recv(POLL) {
                    Some(Event::Data(bytes)) => {
                        self.pending = bytes;
                        break;
                    }
                    Some(Event::Fin | Event::Reset(_) | Event::Closed) => return Ok(0),
                    None => {}
                }
            }
        }
        let n = buf.len().min(self.pending.len());
        buf[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        Ok(n)
    }
}

impl Write for TrunkWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        for chunk in buf.chunks(MAX_CHUNK) {
            self.stream.send(chunk)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for TrunkWriter {
    fn drop(&mut self) {
        self.stream.finish().ok();
    }
}
