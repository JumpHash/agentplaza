//! The daemon's event stream: the frames `GET /api/v0/events` sends, the
//! parser that reads them, and the connection that follows the stream.
//!
//! This is the other half of `crates/noded/src/events.rs`, and speaks exactly
//! the bytes it writes: an HTTP/1.1 head whose `Content-Type` is
//! `text/event-stream`, then `event:`/`data:` frames separated by blank
//! lines, and a `: heartbeat` comment whenever 15 s pass without an event.
//!
//! Two facts shape everything here:
//!
//! * the daemon's bus has **no backlog**, so the stream's head — the status
//!   line and the headers — *is* the subscription's confirmation, and a
//!   subscriber sees only what happens after it. The register's initial state
//!   comes from the GETs, exactly as it did before this module existed;
//! * the stream is the *deltas*, so a stream that ends is not a daemon that
//!   ended: the poll's own health stays the register's source of truth, and
//!   this module's whole job is to reconnect quietly with a bounded backoff
//!   and hand every frame to its caller.
//!
//! Nothing here is iced: the parser is pure and the connection is a blocking
//! loop, so both are unit-tested against real sockets with no display.

use std::fmt;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use serde::Deserialize;

use crate::api::{self, Column, Endpoint, EntryKind, RunState, TaskState, VmState};

/// The path the stream lives at.
pub const PATH: &str = "/api/v0/events";

/// The request the console sends: the one line and the one header the
/// daemon's parser wants. `noded` reads `Host` and never looks at it, and it
/// answers a stream rather than a response, so the connection is the
/// subscription from here on.
const REQUEST: &str = "GET /api/v0/events HTTP/1.1\r\nHost: localhost\r\n\r\n";

/// How long one read waits for bytes before the stream is called stalled. The
/// daemon writes a heartbeat every 15 s, so three heartbeats going by with
/// nothing at all is not a live stream, and reconnecting is the recovery.
pub const READ_TIMEOUT: Duration = Duration::from_secs(45);

/// How long a connection to the stream may take to be established.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The first wait before reconnecting, and the shortest this backoff gets.
pub const BACKOFF_MIN_MS: u64 = 250;

/// The longest wait between reconnections: the same 30 s ceiling the poll's
/// own backoff climbs to, so a daemon that is gone is retried quietly.
pub const BACKOFF_MAX_MS: u64 = 30_000;

/// The most a stream head may occupy, like any other response head.
const MAX_HEAD: usize = api::MAX_HEAD;

/// How much one read takes off the socket.
const CHUNK: usize = 8 * 1024;

/// One event, as the daemon frames it.
///
/// The states reuse the API's own enums, so an event is read exactly as a
/// poll's JSON is: a state this console does not know parses as
/// [`RunState::Unknown`] rather than failing the frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A run's state moved (or the run was registered, as `starting`).
    Run {
        /// The run's id.
        id: String,
        /// The state it moved to.
        state: RunState,
    },
    /// A run's VM changed state.
    Vm {
        /// The run the VM belongs to.
        run: String,
        /// The VM's index.
        vm: u32,
        /// The state it moved to.
        state: VmState,
    },
    /// A run's VM's serial capture grew: `offset` is the byte of the capture
    /// the `text` starts at.
    Serial {
        /// The run the VM belongs to.
        run: String,
        /// The VM's index.
        vm: u32,
        /// The capture byte the text starts at.
        offset: u64,
        /// The bytes written since the last event.
        text: String,
    },
    /// A task moved state (or was created, as `open`): `assignee` is the
    /// schema's `assignee_id` once a claimed agent has one, and `None` while
    /// nobody does.
    ///
    /// The frame carries no clock, no budget and no goal: it is the ledger's
    /// *delta*, and a task this console has not read is read from the ledger
    /// rather than assembled from a frame.
    Task {
        /// The task's id.
        id: String,
        /// The state it moved to.
        state: TaskState,
        /// The schema's `assignee_id` once an agent holds it.
        assignee: Option<String>,
    },
    /// A treasury entry was posted: the double-entry journal grew.
    ///
    /// The frame carries the entry's id, its kind, its size and the task it
    /// belongs to, and **nothing else** — no memo, no postings and no clock.
    /// It is the journal's delta, not an entry: a console that has not read
    /// the journal reads it rather than assembling a row from a frame.
    Treasury {
        /// The entry's id.
        entry: u64,
        /// The schema's entry kind.
        kind: EntryKind,
        /// The entry's size: its credited total, which equals its debited
        /// total.
        amount: i128,
        /// The task the entry belongs to, or `None` when it belongs to none.
        task: Option<String>,
    },
}

/// What one frame of the stream turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// A run, vm or serial event.
    Event(Event),
    /// A comment line: the daemon's heartbeat, and the proof the connection
    /// is alive.
    Comment,
    /// A frame of a kind this console does not watch. Server-sent events
    /// allow any name, so an unknown one is ignored rather than failed.
    Other(String),
    /// A frame of a known kind whose `data` could not be read. It is named
    /// and dropped: one bad frame never ends the stream.
    Malformed(String),
}

/// The incremental server-sent-events parser.
///
/// Bytes go in as they arrive off the socket and complete frames come out, so
/// a line split across two reads is whole when it is parsed. Nothing panics
/// on any input: this is fed by whatever the daemon writes.
#[derive(Debug, Default)]
pub struct Framer {
    /// Bytes read but not yet a whole line.
    buffer: Vec<u8>,
    /// The frame's `event:` name, when it has one.
    kind: Option<String>,
    /// The frame's `data:` lines, in order.
    data: Vec<String>,
}

impl Framer {
    /// A parser with nothing buffered.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds the bytes just read and answers the frames they completed.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Frame> {
        let mut buffer = std::mem::take(&mut self.buffer);
        buffer.extend_from_slice(bytes);

        let mut frames = Vec::new();
        let mut consumed = 0;
        while let Some(at) = buffer[consumed..].iter().position(|byte| *byte == b'\n') {
            let end = consumed + at;
            Self::line(&buffer[consumed..end], &mut self.kind, &mut self.data, &mut frames);
            consumed = end + 1;
        }
        buffer.drain(..consumed);
        self.buffer = buffer;
        frames
    }

    /// Reads one line — without its terminator — into the frame being built.
    fn line(line: &[u8], kind: &mut Option<String>, data: &mut Vec<String>, out: &mut Vec<Frame>) {
        // The daemon terminates frames with LF; a CRLF writer is read too.
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(first) = line.first() else {
            // The blank line ends the frame.
            Self::dispatch(kind, data, out);
            return;
        };
        if *first == b':' {
            out.push(Frame::Comment);
            return;
        }
        let Some(at) = line.iter().position(|byte| *byte == b':') else {
            out.push(Frame::Malformed(format!(
                "a field line without a colon: {:?}",
                String::from_utf8_lossy(line)
            )));
            return;
        };
        let name = &line[..at];
        let value = line[at + 1..].strip_prefix(b" ").unwrap_or(&line[at + 1..]);
        match name {
            b"event" => *kind = Some(String::from_utf8_lossy(value).into_owned()),
            b"data" => data.push(String::from_utf8_lossy(value).into_owned()),
            // `id`, `retry` and anything a newer daemon adds are not this
            // console's business.
            _ => {}
        }
    }

    /// Turns the fields gathered into one frame.
    fn dispatch(kind: &mut Option<String>, data: &mut Vec<String>, out: &mut Vec<Frame>) {
        let kind = kind.take();
        let mut data = std::mem::take(data);
        if kind.is_none() && data.is_empty() {
            // A stray blank line, or the one that follows a comment.
            return;
        }
        // Server-sent events' default name for a frame with no `event:`.
        let kind = kind.unwrap_or_else(|| "message".to_string());
        if data.is_empty() {
            out.push(Frame::Malformed(format!("a {kind} frame with no data")));
            return;
        }
        // One `data:` line is the daemon's whole frame; more than one joins
        // them the way the event-stream format says.
        let payload = if data.len() == 1 {
            data.pop().unwrap_or_default()
        } else {
            data.join("\n")
        };
        let event = match kind.as_str() {
            "run" => serde_json::from_str::<RunData>(&payload)
                .map(|data| Event::Run { id: data.id, state: data.state }),
            "vm" => serde_json::from_str::<VmData>(&payload)
                .map(|data| Event::Vm { run: data.run, vm: data.vm, state: data.state }),
            "serial" => serde_json::from_str::<SerialData>(&payload).map(|data| Event::Serial {
                run: data.run,
                vm: data.vm,
                offset: data.offset,
                text: data.text,
            }),
            "task" => serde_json::from_str::<TaskData>(&payload).map(|data| Event::Task {
                id: data.id,
                state: data.state,
                assignee: data.assignee.into_option(),
            }),
            "treasury" => serde_json::from_str::<TreasuryData>(&payload).map(|data| {
                Event::Treasury {
                    entry: data.entry,
                    kind: data.kind,
                    amount: data.amount,
                    task: data.task.into_option(),
                }
            }),
            other => {
                out.push(Frame::Other(other.to_string()));
                return;
            }
        };
        out.push(match event {
            Ok(event) => Frame::Event(event),
            Err(err) => Frame::Malformed(format!("a {kind} frame: {err}")),
        });
    }
}

/// A `run` frame's `data`.
#[derive(Debug, Deserialize)]
struct RunData {
    id: String,
    state: RunState,
}

/// A `vm` frame's `data`.
#[derive(Debug, Deserialize)]
struct VmData {
    run: String,
    vm: u32,
    state: VmState,
}

/// A `serial` frame's `data`.
#[derive(Debug, Deserialize)]
struct SerialData {
    run: String,
    vm: u32,
    offset: u64,
    text: String,
}

/// A `task` frame's `data`.
///
/// `noded` writes all three fields for every task transition it publishes
/// (`crates/noded/src/events.rs`, `Event::Task::frame`), so an absent
/// `assignee` is not the shape the daemon sends and is refused by name — the
/// same rule the ledger's own rows are read by.
#[derive(Debug, Deserialize)]
struct TaskData {
    id: String,
    state: TaskState,
    assignee: Column<String>,
}

/// A `treasury` frame's `data`.
///
/// `noded` writes all four fields for every entry it publishes
/// (`crates/noded/src/events.rs`, `Event::Treasury::frame`), so an absent
/// `task` is not the shape the daemon sends and is refused by name — the same
/// rule the journal's own rows are read by.
#[derive(Debug, Deserialize)]
struct TreasuryData {
    entry: u64,
    kind: EntryKind,
    amount: i128,
    task: Column<String>,
}

/// Why one connection to the stream did not deliver events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamError {
    /// The daemon could not be reached.
    Connect(String),
    /// The daemon answered, but not with an event stream: an older daemon
    /// that does not serve the endpoint, or something else on the port.
    Head(String),
    /// The connection ended, timed out or broke while reading it.
    Read(String),
}

impl fmt::Display for StreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StreamError::Connect(message) => write!(formatter, "cannot reach {message}"),
            StreamError::Head(message) => write!(formatter, "not an event stream: {message}"),
            StreamError::Read(message) => write!(formatter, "reading the stream: {message}"),
        }
    }
}

impl std::error::Error for StreamError {}

/// An open stream: the socket, the frames read from it so far, and the bytes
/// of the head that turned out to be frames.
#[derive(Debug)]
struct Stream {
    socket: TcpStream,
    framer: Framer,
    pending: Vec<u8>,
}

impl Stream {
    /// The next frames, waiting for bytes if there are none. An error means
    /// the connection is over: end of file, a broken connection, or a read
    /// that waited longer than [`READ_TIMEOUT`] — a stream that has stopped
    /// proving it is alive.
    fn frames(&mut self) -> Result<Vec<Frame>, StreamError> {
        if !self.pending.is_empty() {
            let bytes = std::mem::take(&mut self.pending);
            return Ok(self.framer.feed(&bytes));
        }
        let mut chunk = [0u8; CHUNK];
        let read = self
            .socket
            .read(&mut chunk)
            .map_err(|err| StreamError::Read(err.to_string()))?;
        if read == 0 {
            return Err(StreamError::Read("the daemon closed the stream".to_string()));
        }
        Ok(self.framer.feed(&chunk[..read]))
    }
}

/// Connects to the daemon's stream and reads its head.
///
/// The head is the whole subscription: the daemon answers `200` and
/// `Content-Type: text/event-stream` and then writes frames, so anything the
/// socket already holds past the head is frames already, and is kept for the
/// first [`Stream::frames`].
fn open(endpoint: &Endpoint) -> Result<Stream, StreamError> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, endpoint.port()));
    let mut socket = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT)
        .map_err(|err| StreamError::Connect(format!("{}: {err}", endpoint.url())))?;
    let _ = socket.set_nodelay(true);
    socket
        .set_read_timeout(Some(READ_TIMEOUT))
        .map_err(|err| StreamError::Connect(format!("{}: {err}", endpoint.url())))?;

    socket
        .write_all(REQUEST.as_bytes())
        .and_then(|()| socket.flush())
        .map_err(|err| StreamError::Connect(format!("{}: {err}", endpoint.url())))?;

    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(at) = api::find(&buffer, b"\r\n\r\n") {
            break at + 4;
        }
        if buffer.len() > MAX_HEAD {
            return Err(StreamError::Head("the head is too large".to_string()));
        }
        let read = socket
            .read(&mut chunk)
            .map_err(|err| StreamError::Read(err.to_string()))?;
        if read == 0 {
            return Err(StreamError::Head(
                "the daemon closed before answering".to_string(),
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
    };

    parse_head(&String::from_utf8_lossy(&buffer[..head_end - 4]))?;
    Ok(Stream {
        socket,
        framer: Framer::new(),
        pending: buffer[head_end..].to_vec(),
    })
}

/// Checks a head is a stream: `200`, and `text/event-stream`.
fn parse_head(head: &str) -> Result<(), StreamError> {
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap_or_default();
    if status.split(' ').nth(1) != Some("200") {
        return Err(StreamError::Head(format!("the daemon answered {status:?}")));
    }
    let content_type = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-type")
            .then(|| value.trim().to_string())
    });
    match content_type {
        Some(value) if value.to_ascii_lowercase().starts_with("text/event-stream") => Ok(()),
        Some(value) => Err(StreamError::Head(format!(
            "the daemon answered {value:?}, not text/event-stream"
        ))),
        None => Err(StreamError::Head(
            "the head has no Content-Type".to_string(),
        )),
    }
}

/// How many events one connection delivered, and whether the caller asked for
/// no more.
type Attempt = (u64, bool);

/// Reads one connection to the end, handing every event to `sink`. The first
/// `false` from the sink — the subscription is gone — stops reading at once.
fn read_once(endpoint: &Endpoint, sink: &mut impl FnMut(Event) -> bool) -> Attempt {
    let Ok(mut stream) = open(endpoint) else {
        return (0, false);
    };
    let mut delivered = 0;
    loop {
        let Ok(frames) = stream.frames() else {
            return (delivered, false);
        };
        for frame in frames {
            if let Frame::Event(event) = frame {
                delivered += 1;
                if !sink(event) {
                    return (delivered, true);
                }
            }
        }
    }
}

/// The wait before the next connection, given the one just used and how many
/// events that stream delivered.
///
/// A stream that delivered anything at all proved the daemon can talk to this
/// console, so the ladder starts over; one that delivered nothing doubles the
/// wait, up to [`BACKOFF_MAX_MS`]. Every attempt is itself bounded — the
/// connect timeout and the read timeout — so a daemon that accepts
/// connections and then says nothing cannot hold the thread for longer than
/// [`READ_TIMEOUT`].
pub fn next_wait(previous_ms: u64, delivered: u64) -> u64 {
    if delivered > 0 {
        BACKOFF_MIN_MS
    } else {
        previous_ms.saturating_mul(2).min(BACKOFF_MAX_MS)
    }
}

/// Follows the stream until `sink` answers `false`, reconnecting in between
/// with the bounded backoff above. Returns how many events it delivered.
///
/// This is the whole reconnect story, and it lives here rather than in the
/// subscription because iced 0.14 does not restart a subscription whose
/// stream has completed: the tracker keeps the recipe and never spawns it
/// again. So the stream this module's caller wraps never completes by itself
/// — a stream that ends sleeps, widens its wait and connects again, for as
/// long as anything is listening.
pub fn follow(endpoint: &Endpoint, sink: &mut impl FnMut(Event) -> bool) -> u64 {
    let mut delivered_total = 0;
    let mut wait = BACKOFF_MIN_MS;
    loop {
        let (delivered, stopped) = read_once(endpoint, sink);
        delivered_total += delivered;
        if stopped {
            return delivered_total;
        }
        std::thread::sleep(Duration::from_millis(wait));
        wait = next_wait(wait, delivered);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::net::TcpListener;
    use std::thread;

    /// The head the daemon writes, byte for byte
    /// (`crates/noded/src/events.rs`).
    const HEAD: &str = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                        Cache-Control: no-store\r\nConnection: close\r\n\r\n";

    /// A stub stream: it accepts `connections` connections, reads the request
    /// the way the daemon does, answers each with `head` and `body`, and
    /// closes.
    fn stub(head: &str, body: &str, connections: usize) -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("the stub should bind");
        let port = listener
            .local_addr()
            .expect("a bound listener has an address")
            .port();
        let head = head.to_string();
        let body = body.to_string();
        thread::spawn(move || {
            for stream in listener.incoming().take(connections) {
                let Ok(stream) = stream else { break };
                let mut stream = std::io::BufReader::new(stream);
                // The request: the request line and headers.
                loop {
                    let mut line = String::new();
                    if stream.read_line(&mut line).unwrap_or(0) == 0 || line.trim_end().is_empty() {
                        break;
                    }
                }
                let _ = stream.get_mut().write_all(head.as_bytes());
                let _ = stream.get_mut().write_all(body.as_bytes());
                let _ = stream.get_mut().flush();
                // Dropping the stream closes the connection, which is how a
                // stream ends.
            }
        });
        port
    }

    /// The endpoint of a stub.
    fn endpoint(port: u16) -> Endpoint {
        Endpoint::loopback_at("127.0.0.1", port).expect("loopback")
    }

    #[test]
    fn the_framer_reads_the_daemons_frames() {
        let mut framer = Framer::new();
        let frames = framer.feed(
            b"event: run\ndata: {\"id\":\"r1\",\"state\":\"running\"}\n\n\
              event: vm\ndata: {\"run\":\"r1\",\"vm\":2,\"state\":\"exited\"}\n\n\
              event: serial\ndata: {\"run\":\"r1\",\"vm\":0,\"offset\":16647,\"text\":\"$ uname\\n\"}\n\n",
        );
        assert_eq!(
            frames,
            vec![
                Frame::Event(Event::Run {
                    id: "r1".to_string(),
                    state: RunState::Running,
                }),
                Frame::Event(Event::Vm {
                    run: "r1".to_string(),
                    vm: 2,
                    state: VmState::Exited,
                }),
                Frame::Event(Event::Serial {
                    run: "r1".to_string(),
                    vm: 0,
                    offset: 16_647,
                    text: "$ uname\n".to_string(),
                }),
            ]
        );
    }

    #[test]
    fn a_frame_split_across_reads_is_read_whole() {
        let bytes =
            b"event: serial\ndata: {\"run\":\"r1\",\"vm\":0,\"offset\":0,\"text\":\"hello\"}\n\n";
        let mut framer = Framer::new();
        let mut frames = Vec::new();
        for byte in bytes {
            frames.extend(framer.feed(&[*byte]));
        }
        assert_eq!(
            frames,
            vec![Frame::Event(Event::Serial {
                run: "r1".to_string(),
                vm: 0,
                offset: 0,
                text: "hello".to_string(),
            })],
            "a byte at a time reads the same frame a whole read does"
        );
    }

    #[test]
    fn a_frames_bytes_split_mid_line_across_two_reads_survive() {
        let mut framer = Framer::new();
        assert!(framer.feed(b"event: ru").is_empty());
        assert!(framer.feed(b"n\nda").is_empty());
        assert_eq!(
            framer.feed(b"ta: {\"id\":\"r7\",\"state\":\"done\"}\n\n"),
            vec![Frame::Event(Event::Run {
                id: "r7".to_string(),
                state: RunState::Done,
            })]
        );
    }

    #[test]
    fn a_heartbeat_is_a_comment_and_not_an_event() {
        let mut framer = Framer::new();
        assert_eq!(framer.feed(b": heartbeat\n\n"), vec![Frame::Comment]);
        // And the blank line that follows it is not a frame of its own.
        assert!(framer.feed(b"\n").is_empty());
        assert!(framer.feed(b"").is_empty());
    }

    #[test]
    fn a_malformed_frame_is_named_and_the_next_one_still_reads() {
        let mut framer = Framer::new();
        let frames = framer.feed(
            b"event: serial\ndata: {oops\n\n\
              event: run\ndata: {\"id\":\"r1\",\"state\":\"done\"}\n\n",
        );
        assert_eq!(frames.len(), 2);
        assert!(
            matches!(&frames[0], Frame::Malformed(why) if why.contains("serial")),
            "the broken frame is named, got {:?}",
            frames[0]
        );
        assert_eq!(
            frames[1],
            Frame::Event(Event::Run {
                id: "r1".to_string(),
                state: RunState::Done,
            })
        );
    }

    #[test]
    fn a_frame_with_no_data_is_named() {
        let mut framer = Framer::new();
        assert_eq!(
            framer.feed(b"event: run\n\n"),
            vec![Frame::Malformed("a run frame with no data".to_string())]
        );
    }

    #[test]
    fn an_unknown_kind_is_ignored_rather_than_failed() {
        let mut framer = Framer::new();
        assert_eq!(
            framer.feed(b"event: log\ndata: {\"line\":\"hi\"}\n\n"),
            vec![Frame::Other("log".to_string())]
        );
    }

    #[test]
    fn a_data_line_with_no_kind_is_the_default_message() {
        let mut framer = Framer::new();
        assert_eq!(
            framer.feed(b"data: {}\n\n"),
            vec![Frame::Other("message".to_string())]
        );
    }

    #[test]
    fn a_line_with_no_colon_is_named() {
        let mut framer = Framer::new();
        assert!(
            matches!(framer.feed(b"garbage\ndata: {}\n\n").first(), Some(Frame::Malformed(why)) if why.contains("garbage")),
        );
    }

    #[test]
    fn id_and_retry_fields_are_not_the_consoles_business() {
        let mut framer = Framer::new();
        assert_eq!(
            framer.feed(b"event: run\nid: 7\nretry: 100\ndata: {\"id\":\"r1\",\"state\":\"killed\"}\n\n"),
            vec![Frame::Event(Event::Run {
                id: "r1".to_string(),
                state: RunState::Killed,
            })]
        );
    }

    #[test]
    fn crlf_frames_and_a_leading_blank_line_are_read() {
        let mut framer = Framer::new();
        let frames = framer.feed(b"\r\nevent: vm\r\ndata: {\"run\":\"r1\",\"vm\":0,\"state\":\"running\"}\r\n\r\n");
        assert_eq!(
            frames,
            vec![Frame::Event(Event::Vm {
                run: "r1".to_string(),
                vm: 0,
                state: VmState::Running,
            })]
        );
    }

    #[test]
    fn a_states_name_this_console_does_not_know_parses_as_unknown() {
        let mut framer = Framer::new();
        assert_eq!(
            framer.feed(b"event: run\ndata: {\"id\":\"r1\",\"state\":\"hibernating\"}\n\n"),
            vec![Frame::Event(Event::Run {
                id: "r1".to_string(),
                state: RunState::Unknown,
            })]
        );
    }

    #[test]
    fn a_serial_frame_carries_its_bytes_exactly() {
        let mut framer = Framer::new();
        let frames = framer.feed(
            b"event: serial\ndata: {\"run\":\"r1\",\"vm\":1,\"offset\":9,\"text\":\"a\\\"b\\\\c\\nline\\u0007 \\u00e9\"}\n\n",
        );
        assert_eq!(
            frames,
            vec![Frame::Event(Event::Serial {
                run: "r1".to_string(),
                vm: 1,
                offset: 9,
                text: "a\"b\\c\nline\u{7} é".to_string(),
            })]
        );
    }

    #[test]
    fn the_framer_reads_a_task_frame_the_daemon_writes() {
        // `crates/noded/src/events.rs`'s own frames, byte for byte: a create
        // (assignee null) and a claim (assignee is the schema's
        // `assignee_id`).
        let mut framer = Framer::new();
        let frames = framer.feed(
            b"event: task\ndata: {\"id\":\"t1\",\"state\":\"open\",\"assignee\":null}\n\n\
              event: task\ndata: {\"id\":\"t1\",\"state\":\"claimed\",\"assignee\":\"librarian\"}\n\n",
        );
        assert_eq!(
            frames,
            vec![
                Frame::Event(Event::Task {
                    id: "t1".to_string(),
                    state: TaskState::Open,
                    assignee: None,
                }),
                Frame::Event(Event::Task {
                    id: "t1".to_string(),
                    state: TaskState::Claimed,
                    assignee: Some("librarian".to_string()),
                }),
            ]
        );
    }

    #[test]
    fn a_task_frame_with_a_state_this_console_does_not_know_parses_as_unknown() {
        let mut framer = Framer::new();
        assert_eq!(
            framer.feed(b"event: task\ndata: {\"id\":\"t1\",\"state\":\"verifying\",\"assignee\":null}\n\n"),
            vec![Frame::Event(Event::Task {
                id: "t1".to_string(),
                state: TaskState::Unknown,
                assignee: None,
            })]
        );
    }

    #[test]
    fn a_task_frame_missing_a_field_is_named_and_the_next_frame_still_reads() {
        let mut framer = Framer::new();
        let frames = framer.feed(
            b"event: task\ndata: {\"id\":\"t1\",\"assignee\":null}\n\n\
              event: task\ndata: {\"id\":\"t1\",\"state\":\"open\"}\n\n\
              event: task\ndata: {oops\n\n\
              event: run\ndata: {\"id\":\"r1\",\"state\":\"done\"}\n\n",
        );
        assert_eq!(frames.len(), 4);
        assert!(
            matches!(&frames[0], Frame::Malformed(why) if why.contains("task") && why.contains("state")),
            "a task frame with no state is named: {:?}",
            frames[0]
        );
        assert!(
            matches!(&frames[1], Frame::Malformed(why) if why.contains("task") && why.contains("assignee")),
            "a task frame with no assignee is named: {:?}",
            frames[1]
        );
        assert!(matches!(&frames[2], Frame::Malformed(why) if why.contains("task")));
        assert_eq!(
            frames[3],
            Frame::Event(Event::Run {
                id: "r1".to_string(),
                state: RunState::Done,
            }),
            "one bad task frame never ends the stream"
        );
    }

    #[test]
    fn the_framer_reads_a_treasury_frame_the_daemon_writes() {
        // `crates/noded/src/events.rs`'s own frames, byte for byte: a task's
        // mint (the task is named) and a fine (it is null).
        let mut framer = Framer::new();
        let frames = framer.feed(
            b"event: treasury\ndata: {\"entry\":1,\"kind\":\"mint\",\"amount\":500,\"task\":\"t1\"}\n\n\
              event: treasury\ndata: {\"entry\":7,\"kind\":\"fine\",\"amount\":5,\"task\":null}\n\n",
        );
        assert_eq!(
            frames,
            vec![
                Frame::Event(Event::Treasury {
                    entry: 1,
                    kind: EntryKind::Mint,
                    amount: 500,
                    task: Some("t1".to_string()),
                }),
                Frame::Event(Event::Treasury {
                    entry: 7,
                    kind: EntryKind::Fine,
                    amount: 5,
                    task: None,
                }),
            ]
        );
    }

    #[test]
    fn a_treasury_frame_with_a_kind_this_console_does_not_know_parses_as_unknown() {
        let mut framer = Framer::new();
        assert_eq!(
            framer.feed(b"event: treasury\ndata: {\"entry\":3,\"kind\":\"tithe\",\"amount\":1,\"task\":null}\n\n"),
            vec![Frame::Event(Event::Treasury {
                entry: 3,
                kind: EntryKind::Unknown,
                amount: 1,
                task: None,
            })]
        );
    }

    #[test]
    fn a_treasury_frame_missing_a_field_is_named_and_the_next_frame_still_reads() {
        let mut framer = Framer::new();
        let frames = framer.feed(
            b"event: treasury\ndata: {\"entry\":1,\"kind\":\"mint\",\"amount\":5}\n\n\
              event: treasury\ndata: {\"entry\":1,\"kind\":\"mint\",\"task\":null}\n\n\
              event: run\ndata: {\"id\":\"r1\",\"state\":\"done\"}\n\n",
        );
        assert_eq!(frames.len(), 3);
        assert!(
            matches!(&frames[0], Frame::Malformed(why) if why.contains("treasury") && why.contains("task")),
            "a treasury frame with no task is named: {:?}",
            frames[0]
        );
        assert!(
            matches!(&frames[1], Frame::Malformed(why) if why.contains("treasury") && why.contains("amount")),
            "a treasury frame with no amount is named: {:?}",
            frames[1]
        );
        assert_eq!(
            frames[2],
            Frame::Event(Event::Run {
                id: "r1".to_string(),
                state: RunState::Done,
            }),
            "one bad treasury frame never ends the stream"
        );
    }

    #[test]
    fn the_backoff_ladder_doubles_to_the_ceiling_and_resets_on_a_live_stream() {
        assert_eq!(BACKOFF_MIN_MS, 250, "the first wait after a stream ends");
        let mut wait = BACKOFF_MIN_MS;
        for expected in [500, 1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000] {
            wait = next_wait(wait, 0);
            assert_eq!(wait, expected, "the ladder after a stream that delivered nothing");
        }
        assert_eq!(next_wait(wait, 1), BACKOFF_MIN_MS, "one event resets the ladder");
        assert_eq!(next_wait(BACKOFF_MIN_MS, 1), BACKOFF_MIN_MS);
    }

    #[test]
    fn the_head_is_the_subscriptions_confirmation() {
        let port = stub(HEAD, "", 1);
        let stream = open(&endpoint(port)).expect("the stub answers a stream");
        assert!(stream.pending.is_empty(), "the head carried no frames");
    }

    #[test]
    fn a_head_that_is_not_a_stream_is_refused() {
        for (head, why) in [
            (
                "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: 0\r\n\r\n",
                "the status",
            ),
            (
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n",
                "the content type",
            ),
            ("HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n", "no content type at all"),
        ] {
            let port = stub(head, "", 1);
            let error = open(&endpoint(port)).expect_err(why);
            assert!(matches!(error, StreamError::Head(_)), "{why}: {error:?}");
        }
    }

    #[test]
    fn bytes_the_head_read_past_are_still_frames() {
        // The daemon can write the head and the first frame in one go, and a
        // socket read can hand both over together.
        let port = stub(
            HEAD,
            "event: run\ndata: {\"id\":\"r1\",\"state\":\"starting\"}\n\n: heartbeat\n\n",
            1,
        );
        let mut stream = open(&endpoint(port)).expect("the stub answers a stream");
        let mut frames = Vec::new();
        while let Ok(more) = stream.frames() {
            frames.extend(more);
        }
        assert_eq!(
            frames,
            vec![
                Frame::Event(Event::Run {
                    id: "r1".to_string(),
                    state: RunState::Starting,
                }),
                Frame::Comment,
            ]
        );
    }

    #[test]
    fn a_stream_that_ends_is_reconnected_and_the_events_keep_coming() {
        // Two connections, each with one run event: the caller stops after
        // the second, so `follow` has to have reconnected to deliver it.
        let body = "event: run\ndata: {\"id\":\"r1\",\"state\":\"running\"}\n\n";
        let port = stub(HEAD, body, 2);
        let endpoint = endpoint(port);
        let mut seen = Vec::new();
        let delivered = follow(&endpoint, &mut |event| {
            seen.push(event);
            seen.len() < 2
        });
        assert_eq!(delivered, 2, "both events arrived");
        assert_eq!(
            seen,
            vec![
                Event::Run {
                    id: "r1".to_string(),
                    state: RunState::Running,
                },
                Event::Run {
                    id: "r1".to_string(),
                    state: RunState::Running,
                },
            ],
            "the same frame came off two connections"
        );
    }

    #[test]
    fn a_stream_that_never_opens_a_socket_delivers_nothing_and_keeps_trying() {
        // A port with no listener: the attempt fails, and the sink is never
        // called, so `follow` is only ever left by the sink — which is why
        // the register's thread ends when its subscription goes away.
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("a bound listener");
        let port = listener
            .local_addr()
            .expect("a bound listener has an address")
            .port();
        drop(listener);

        let mut calls = 0;
        let attempt = read_once(&endpoint(port), &mut |_| {
            calls += 1;
            true
        });
        assert_eq!(attempt, (0, false), "nothing was delivered and the sink never said stop");
        assert_eq!(calls, 0);
    }
}
