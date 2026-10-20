//! Blocking JSON-RPC 2.0 over a line-delimited byte stream, which is how ACP travels (one JSON object
//! per line in each direction). It is owned here, not taken from an SDK, because the guest harness has no
//! async runtime and the hub already runs its sockets on threads.
//!
//! A [`Peer`] owns one stream in each direction. One reader thread splits the input into lines and
//! sorts them. One dispatcher thread takes the sorted items in arrival order, so a reply is delivered
//! after the notifications the other side sent before it. Each incoming request runs on a thread of its
//! own: a handler may call back into the same peer (a permission request raised while a prompt is being
//! answered) without stalling the stream that carries the answer.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use crate::types;

/// Longest line either side may send, in bytes without its newline.
///
/// The other side is a child process (an agent) or an editor, so its output is untrusted: a longer line
/// is refused and skipped, never held, so memory cannot grow with what the peer writes.
pub const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;

/// Requests from the other side that may be running at once. Each one runs on a thread of its own, so this
/// bounds the threads an untrusted peer can make us start; a request over the bound is answered with an error.
const MAX_IN_FLIGHT_REQUESTS: usize = 64;

/// Inbound items the reader may queue ahead of the dispatcher. A full queue stops the reader, so a peer that
/// writes faster than we handle its messages is slowed down by the pipe, not buffered without limit.
const QUEUE_CAPACITY: usize = 1024;

/// What a [`Peer::call`] can fail with.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The stream ended, or the peer was closed, before a reply came.
    #[error("the connection is closed")]
    Closed,
    /// No reply came within the timeout. A reply that arrives later is dropped.
    #[error("no reply within the timeout")]
    Timeout,
    /// The other side answered with a JSON-RPC error object; its code, message and data are kept.
    #[error("{0}")]
    Remote(types::Error),
    /// The request could not be written to the stream.
    #[error("write failed: {0}")]
    Io(#[from] io::Error),
}

/// Answers a request from the other side. It runs on its own thread, and the value or error it returns is
/// the reply. The peer is passed in so that the handler can call back while it works.
pub type RequestHandler = dyn Fn(&Peer, &str, Value) -> Result<Value, types::Error> + Send + Sync;

/// Handles a notification from the other side. It runs on the dispatcher thread, in arrival order, so it
/// must not wait for a reply from the peer: that reply could not be delivered until it returns.
pub type NotificationHandler = dyn Fn(&Peer, &str, Value) + Send + Sync;

/// One JSON-RPC connection. Cheap to clone: clones share the streams and the pending calls.
#[derive(Clone)]
pub struct Peer {
    inner: Arc<Inner>,
}

struct Inner {
    writer: Mutex<Box<dyn Write + Send>>,
    state: Mutex<State>,
    next_id: AtomicU64,
    in_flight: AtomicUsize,
    on_request: Arc<RequestHandler>,
    on_notification: Arc<NotificationHandler>,
}

#[derive(Default)]
struct State {
    closed: bool,
    pending: HashMap<u64, Sender<Result<Value, Error>>>,
}

/// One item the reader thread hands the dispatcher, in the order the stream carried it.
enum Inbound {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Reply {
        id: u64,
        result: Result<Value, types::Error>,
    },
    End,
}

impl Peer {
    /// Starts a connection over `reader` and `writer`. It returns at once; the threads run until the reader
    /// reaches the end of its stream, or the peer is closed.
    pub fn new<R, W, Q, N>(reader: R, writer: W, on_request: Q, on_notification: N) -> Self
    where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
        Q: Fn(&Peer, &str, Value) -> Result<Value, types::Error> + Send + Sync + 'static,
        N: Fn(&Peer, &str, Value) + Send + Sync + 'static,
    {
        let writer: Box<dyn Write + Send> = Box::new(writer);
        let on_request: Arc<RequestHandler> = Arc::new(on_request);
        let on_notification: Arc<NotificationHandler> = Arc::new(on_notification);
        let inner = Arc::new(Inner {
            writer: Mutex::new(writer),
            state: Mutex::new(State::default()),
            next_id: AtomicU64::new(1),
            in_flight: AtomicUsize::new(0),
            on_request,
            on_notification,
        });
        let (tx, rx) = mpsc::sync_channel(QUEUE_CAPACITY);
        let reader_inner = Arc::clone(&inner);
        thread::Builder::new()
            .name("apz-acp-reader".into())
            .spawn(move || read_loop(BufReader::new(reader), &reader_inner, &tx))
            .expect("apz-acp: cannot start the reader thread");
        let dispatch_inner = Arc::clone(&inner);
        thread::Builder::new()
            .name("apz-acp-dispatch".into())
            .spawn(move || dispatch_loop(&dispatch_inner, &rx))
            .expect("apz-acp: cannot start the dispatcher thread");
        Self { inner }
    }

    /// Sends a request and blocks until its reply arrives, the stream ends, or `timeout` passes. A null
    /// `params` is left out of the message, as JSON-RPC allows.
    pub fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, Error> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        {
            let mut state = lock(&self.inner.state);
            if state.closed {
                return Err(Error::Closed);
            }
            state.pending.insert(id, tx);
        }
        if let Err(err) = self
            .inner
            .write(&request_message(json!(id), method, params))
        {
            lock(&self.inner.state).pending.remove(&id);
            return Err(Error::Io(err));
        }
        match rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                lock(&self.inner.state).pending.remove(&id);
                Err(Error::Timeout)
            }
            Err(RecvTimeoutError::Disconnected) => Err(Error::Closed),
        }
    }

    /// Sends a notification. No reply is expected, and none is waited for.
    pub fn notify(&self, method: &str, params: Value) -> Result<(), Error> {
        if lock(&self.inner.state).closed {
            return Err(Error::Closed);
        }
        self.inner
            .write(&notification_message(method, params))
            .map_err(Error::Io)
    }

    /// Closes the connection from this side: every pending call fails with [`Error::Closed`], and new calls
    /// do too. The reader thread stops at its next line; ending the streams is the owner's job.
    pub fn close(&self) {
        self.inner.close();
    }

    /// Whether the connection is closed, by [`Peer::close`] or by the end of the input stream.
    pub fn is_closed(&self) -> bool {
        lock(&self.inner.state).closed
    }
}

impl Inner {
    /// Writes one message as one line. The writer lock keeps two threads' lines from interleaving.
    fn write(&self, message: &Value) -> io::Result<()> {
        let mut line = serde_json::to_vec(message).map_err(io::Error::other)?;
        line.push(b'\n');
        let mut writer = lock(&self.writer);
        writer.write_all(&line)?;
        writer.flush()
    }

    fn close(&self) {
        let pending = {
            let mut state = lock(&self.state);
            state.closed = true;
            std::mem::take(&mut state.pending)
        };
        for (_, waiter) in pending {
            let _ = waiter.send(Err(Error::Closed));
        }
    }
}

/// Locks a mutex, ignoring poisoning: a panicking handler must not take the whole connection down with it.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn request_message(id: Value, method: &str, params: Value) -> Value {
    let mut message = json!({"jsonrpc": "2.0", "id": id, "method": method});
    if !params.is_null() {
        message["params"] = params;
    }
    message
}

fn notification_message(method: &str, params: Value) -> Value {
    let mut message = json!({"jsonrpc": "2.0", "method": method});
    if !params.is_null() {
        message["params"] = params;
    }
    message
}

/// One line of input, or the reason there is none.
enum Line {
    /// The stream ended with nothing more to read.
    End,
    /// The line was longer than the limit; it was consumed and discarded.
    TooLong,
    Text(Vec<u8>),
}

/// Reads one line without its newline, holding at most `max` bytes of it. A longer line is consumed up to
/// its newline and reported as [`Line::TooLong`], so the stream stays in step with the messages.
fn read_line_bounded<R: BufRead>(reader: &mut R, max: usize) -> io::Result<Line> {
    let mut line = Vec::new();
    let mut too_long = false;
    loop {
        let (finished, used) = {
            let available = reader.fill_buf()?;
            if available.is_empty() {
                // A final line with no newline still counts.
                return Ok(if too_long {
                    Line::TooLong
                } else if line.is_empty() {
                    Line::End
                } else {
                    Line::Text(line)
                });
            }
            let (chunk, finished) = match available.iter().position(|&byte| byte == b'\n') {
                Some(at) => (&available[..at], true),
                None => (available, false),
            };
            if !too_long {
                if line.len() + chunk.len() > max {
                    too_long = true;
                    line = Vec::new();
                } else {
                    line.extend_from_slice(chunk);
                }
            }
            (
                finished,
                if finished {
                    chunk.len() + 1
                } else {
                    chunk.len()
                },
            )
        };
        reader.consume(used);
        if finished {
            return Ok(if too_long {
                Line::TooLong
            } else {
                Line::Text(line)
            });
        }
    }
}

fn read_loop<R: BufRead>(mut reader: R, inner: &Inner, tx: &SyncSender<Inbound>) {
    loop {
        match read_line_bounded(&mut reader, MAX_LINE_BYTES) {
            Ok(Line::Text(mut bytes)) => {
                if bytes.last() == Some(&b'\r') {
                    bytes.pop();
                }
                if bytes.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                handle_line(inner, &bytes, tx);
            }
            Ok(Line::TooLong) => {
                // The line's id is unknown (it was never read), so the error goes out with a null id.
                eprintln!("apz-acp: refused a line over {MAX_LINE_BYTES} bytes");
                let error = types::Error::invalid_request()
                    .data(json!({"reason": "line too long", "limit": MAX_LINE_BYTES}));
                send_reply(inner, &Value::Null, Err(error));
            }
            Ok(Line::End) => break,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => {
                eprintln!("apz-acp: read failed: {err}");
                break;
            }
        }
        if lock(&inner.state).closed {
            break;
        }
    }
    let _ = tx.send(Inbound::End);
}

/// Sorts one line into an inbound item, or answers it with an error. A line that is not JSON has no id to
/// answer, so it is logged and ignored. A JSON message that cannot be used is answered with an invalid
/// request error when it carries a usable id, and otherwise logged and ignored.
fn handle_line(inner: &Inner, line: &[u8], tx: &SyncSender<Inbound>) {
    let message: Value = match serde_json::from_slice(line) {
        Ok(message) => message,
        Err(err) => {
            eprintln!("apz-acp: ignored a line that is not JSON: {err}");
            return;
        }
    };
    let Some(object) = message.as_object() else {
        return reject(inner, None, "a message must be a JSON object");
    };
    let id = object
        .get("id")
        .filter(|id| id.is_string() || id.is_number())
        .cloned();
    let has_id = object.contains_key("id");

    if let Some(method) = object.get("method") {
        let Some(method) = method.as_str() else {
            return reject(inner, id, "method must be a string");
        };
        let params = object.get("params").cloned().unwrap_or(Value::Null);
        let item = if !has_id {
            Inbound::Notification {
                method: method.to_owned(),
                params,
            }
        } else if let Some(id) = id {
            Inbound::Request {
                id,
                method: method.to_owned(),
                params,
            }
        } else {
            return reject(inner, None, "id must be a string or a number");
        };
        let _ = tx.send(item);
        return;
    }

    if object.contains_key("result") || object.contains_key("error") {
        let Some(number) = id.as_ref().and_then(Value::as_u64) else {
            eprintln!("apz-acp: ignored a reply without a numeric id");
            return;
        };
        let result = match object.get("error") {
            Some(error) => Err(serde_json::from_value::<types::Error>(error.clone())
                .unwrap_or_else(|err| {
                    types::Error::internal_error()
                        .data(json!(format!("malformed error object: {err}")))
                })),
            None => Ok(object.get("result").cloned().unwrap_or(Value::Null)),
        };
        let _ = tx.send(Inbound::Reply { id: number, result });
        return;
    }

    reject(inner, id, "neither a request, a notification nor a reply")
}

/// Answers a message we cannot use with an invalid request error, when it has an id to answer; otherwise
/// logs it.
fn reject(inner: &Inner, id: Option<Value>, reason: &str) {
    match id {
        Some(id) => {
            let error = types::Error::invalid_request().data(json!({"reason": reason}));
            send_reply(inner, &id, Err(error));
        }
        None => eprintln!("apz-acp: ignored a message without a usable id: {reason}"),
    }
}

fn send_reply(inner: &Inner, id: &Value, outcome: Result<Value, types::Error>) {
    let reply = match outcome {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
    };
    if let Err(err) = inner.write(&reply) {
        eprintln!("apz-acp: could not send a reply: {err}");
    }
}

fn dispatch_loop(inner: &Arc<Inner>, rx: &Receiver<Inbound>) {
    let peer = Peer {
        inner: Arc::clone(inner),
    };
    while let Ok(item) = rx.recv() {
        match item {
            Inbound::Reply { id, result } => {
                let waiter = lock(&inner.state).pending.remove(&id);
                match waiter {
                    Some(waiter) => {
                        let _ = waiter.send(result.map_err(Error::Remote));
                    }
                    None => eprintln!("apz-acp: ignored a reply to id {id}, which is not pending"),
                }
            }
            Inbound::Notification { method, params } => {
                let handler = Arc::clone(&inner.on_notification);
                let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
                    handler(&peer, &method, params);
                }));
                if outcome.is_err() {
                    eprintln!("apz-acp: the handler for notification {method} panicked");
                }
            }
            Inbound::Request { id, method, params } => {
                // The other side is untrusted, so a flood of requests is refused rather than given a thread each.
                if inner.in_flight.fetch_add(1, Ordering::SeqCst) >= MAX_IN_FLIGHT_REQUESTS {
                    inner.in_flight.fetch_sub(1, Ordering::SeqCst);
                    let error = types::Error::internal_error()
                        .data(json!({"reason": "too many requests in flight"}));
                    send_reply(inner, &id, Err(error));
                    continue;
                }
                let peer = peer.clone();
                let counted = Arc::clone(inner);
                let spawned =
                    thread::Builder::new()
                        .name("apz-acp-request".into())
                        .spawn(move || {
                            answer(&peer, &id, &method, params);
                            counted.in_flight.fetch_sub(1, Ordering::SeqCst);
                        });
                if let Err(err) = spawned {
                    inner.in_flight.fetch_sub(1, Ordering::SeqCst);
                    eprintln!("apz-acp: cannot start a thread for a request: {err}");
                }
            }
            Inbound::End => break,
        }
    }
    peer.close();
}

/// Runs one incoming request and sends its reply. A panic in the handler becomes an internal error reply,
/// so the other side always gets an answer.
fn answer(peer: &Peer, id: &Value, method: &str, params: Value) {
    let handler = Arc::clone(&peer.inner.on_request);
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| handler(peer, method, params)))
        .unwrap_or_else(|_| {
            Err(types::Error::internal_error().data(json!("the request handler panicked")))
        });
    send_reply(&peer.inner, id, outcome);
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAIT: Duration = Duration::from_secs(5);

    /// A reader fed by the test: each chunk sent is one `read`, so tests control where lines split.
    /// Dropping the sender ends the stream.
    struct Pipe {
        chunks: Receiver<Vec<u8>>,
        current: Vec<u8>,
        at: usize,
    }

    impl Read for Pipe {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.at == self.current.len() {
                match self.chunks.recv() {
                    Ok(chunk) => {
                        self.current = chunk;
                        self.at = 0;
                    }
                    Err(_) => return Ok(0),
                }
            }
            let n = buf.len().min(self.current.len() - self.at);
            buf[..n].copy_from_slice(&self.current[self.at..self.at + n]);
            self.at += n;
            Ok(n)
        }
    }

    /// A writer that hands each message (one `write_all`) to the test.
    struct Sink(Sender<Vec<u8>>);

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let _ = self.0.send(buf.to_vec());
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct Harness {
        input: Sender<Vec<u8>>,
        output: Receiver<Vec<u8>>,
        peer: Peer,
        notes: Receiver<(String, Value)>,
    }

    /// A peer whose request handler echoes the params back, unless the method is "fail" (an error) or
    /// "boom" (a panic). Notifications are passed to the test through `notes`.
    fn harness() -> Harness {
        harness_with_hold(None)
    }

    fn harness_with_hold(hold: Option<Arc<(Mutex<bool>, std::sync::Condvar)>>) -> Harness {
        let (input, chunks) = mpsc::channel();
        let (out_tx, output) = mpsc::channel();
        let (note_tx, notes) = mpsc::channel();
        let peer = Peer::new(
            Pipe {
                chunks,
                current: Vec::new(),
                at: 0,
            },
            Sink(out_tx),
            move |_peer, method, params| match method {
                "fail" => Err(types::Error::new(-32000, "refused").data(json!({"why": "test"}))),
                "boom" => panic!("handler failure on purpose"),
                "hold" => {
                    let (released, wake) = hold.as_ref().expect("hold gate provided").as_ref();
                    let (released, _) = wake
                        .wait_timeout_while(lock(released), WAIT, |released| !*released)
                        .unwrap();
                    assert!(*released, "test did not release the held requests");
                    Ok(json!("held"))
                }
                _ => Ok(json!({"echo": params})),
            },
            move |_peer, method, params| {
                let _ = note_tx.send((method.to_owned(), params));
            },
        );
        Harness {
            input,
            output,
            peer,
            notes,
        }
    }

    fn feed(harness: &Harness, text: &str) {
        harness.input.send(text.as_bytes().to_vec()).unwrap();
    }

    fn next_message(harness: &Harness) -> Value {
        let bytes = harness.output.recv_timeout(WAIT).expect("a message");
        serde_json::from_slice(bytes.strip_suffix(b"\n").unwrap_or(&bytes)).expect("json")
    }

    fn no_message(harness: &Harness) {
        assert!(
            harness
                .output
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        );
    }

    #[test]
    fn framing_splits_joins_and_skips_lines() {
        // A line split across two reads is one message.
        {
            let harness = harness();
            feed(&harness, "{\"jsonrpc\":\"2.0\",\"method\":\"note\",");
            feed(&harness, "\"params\":{\"n\":1}}\n");
            let (method, params) = harness.notes.recv_timeout(WAIT).unwrap();
            assert_eq!(method, "note");
            assert_eq!(params, json!({"n": 1}));
            no_message(&harness);
        }

        // Two messages in one read are delivered in order.
        {
            let harness = harness();
            feed(
                &harness,
                "{\"jsonrpc\":\"2.0\",\"method\":\"a\"}\n{\"jsonrpc\":\"2.0\",\"method\":\"b\"}\r\n",
            );
            assert_eq!(harness.notes.recv_timeout(WAIT).unwrap().0, "a");
            assert_eq!(harness.notes.recv_timeout(WAIT).unwrap().0, "b");
        }

        // A final line without a newline is still read.
        {
            let harness = harness();
            feed(&harness, "{\"jsonrpc\":\"2.0\",\"method\":\"last\"}");
            // Only the end of the stream completes a line that has no newline.
            let Harness { input, notes, .. } = harness;
            drop(input);
            assert_eq!(notes.recv_timeout(WAIT).unwrap().0, "last");
        }

        // A line that is only whitespace is skipped.
        {
            let harness = harness();
            feed(
                &harness,
                "   \r\n\n{\"jsonrpc\":\"2.0\",\"method\":\"note\"}\n",
            );
            assert_eq!(harness.notes.recv_timeout(WAIT).unwrap().0, "note");
            no_message(&harness);
        }
    }

    #[test]
    fn a_request_is_answered_with_its_id_and_the_handler_result() {
        let harness = harness();
        feed(
            &harness,
            "{\"jsonrpc\":\"2.0\",\"id\":\"req-1\",\"method\":\"echo\",\"params\":{\"x\":2}}\n",
        );
        let reply = next_message(&harness);
        assert_eq!(reply["id"], json!("req-1"));
        assert_eq!(reply["result"], json!({"echo": {"x": 2}}));
    }

    #[test]
    fn a_handler_error_and_a_handler_panic_are_answered_as_errors() {
        let harness = harness();
        feed(
            &harness,
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"fail\"}\n",
        );
        let reply = next_message(&harness);
        assert_eq!(reply["id"], json!(1));
        assert_eq!(reply["error"]["code"], json!(-32000));
        assert_eq!(reply["error"]["data"], json!({"why": "test"}));

        feed(
            &harness,
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"boom\"}\n",
        );
        let reply = next_message(&harness);
        assert_eq!(reply["id"], json!(2));
        assert_eq!(reply["error"]["code"], json!(-32603));
    }

    #[test]
    fn a_notification_gets_no_reply() {
        let harness = harness();
        feed(
            &harness,
            "{\"jsonrpc\":\"2.0\",\"method\":\"note\",\"params\":[1]}\n",
        );
        assert_eq!(harness.notes.recv_timeout(WAIT).unwrap().0, "note");
        no_message(&harness);
    }

    #[test]
    fn invalid_json_is_ignored_even_when_it_looks_like_it_has_an_id() {
        let harness = harness();
        feed(&harness, "{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\n");
        feed(&harness, "not json at all\n");
        no_message(&harness);
        // The stream is still in step: the next valid request is answered.
        feed(
            &harness,
            "{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"echo\"}\n",
        );
        assert_eq!(next_message(&harness)["id"], json!(3));
    }

    #[test]
    fn a_json_message_that_is_not_valid_json_rpc_is_rejected_only_with_an_id() {
        let harness = harness();
        feed(&harness, "{\"jsonrpc\":\"2.0\",\"id\":9}\n");
        let reply = next_message(&harness);
        assert_eq!(reply["id"], json!(9));
        assert_eq!(reply["error"]["code"], json!(-32600));

        feed(&harness, "{\"jsonrpc\":\"2.0\",\"method\":5}\n");
        no_message(&harness);
    }

    #[test]
    fn requests_beyond_the_in_flight_limit_are_refused_at_once() {
        let held = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let harness = harness_with_hold(Some(Arc::clone(&held)));
        for id in 1..=MAX_IN_FLIGHT_REQUESTS {
            feed(
                &harness,
                &format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"hold\"}}\n"),
            );
        }
        feed(
            &harness,
            "{\"jsonrpc\":\"2.0\",\"id\":\"extra\",\"method\":\"echo\"}\n",
        );
        // No held handler can finish until the overflow response has arrived.
        let refused = next_message(&harness);
        *lock(&held.0) = true;
        held.1.notify_all();
        assert_eq!(refused["id"], json!("extra"));
        assert_eq!(refused["error"]["code"], json!(-32603));
        for _ in 0..MAX_IN_FLIGHT_REQUESTS {
            assert_eq!(next_message(&harness)["result"], json!("held"));
        }
    }

    #[test]
    fn an_over_long_line_is_refused_and_the_stream_goes_on() {
        let harness = harness();
        let mut long = String::from("{\"jsonrpc\":\"2.0\",\"method\":\"note\",\"params\":\"");
        long.push_str(&"x".repeat(MAX_LINE_BYTES));
        long.push_str("\"}\n");
        // Sent in pieces, as a real pipe delivers it.
        for piece in long.as_bytes().chunks(1 << 20) {
            harness.input.send(piece.to_vec()).unwrap();
        }
        let reply = next_message(&harness);
        assert_eq!(reply["id"], Value::Null);
        assert_eq!(reply["error"]["code"], json!(-32600));
        assert!(
            harness
                .notes
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        );

        feed(&harness, "{\"jsonrpc\":\"2.0\",\"method\":\"after\"}\n");
        assert_eq!(harness.notes.recv_timeout(WAIT).unwrap().0, "after");
    }

    #[test]
    fn out_of_order_replies_reach_the_right_callers() {
        let harness = harness();
        let first = harness.peer.clone();
        let caller = thread::spawn(move || first.call("first", json!(1), WAIT));
        let request_a = next_message(&harness);
        let second = harness.peer.clone();
        let caller_two = thread::spawn(move || second.call("second", json!(2), WAIT));
        let request_b = next_message(&harness);
        assert_eq!(request_a["method"], json!("first"));
        assert_eq!(request_b["method"], json!("second"));
        let (id_a, id_b) = (request_a["id"].clone(), request_b["id"].clone());

        feed(
            &harness,
            &format!("{{\"jsonrpc\":\"2.0\",\"id\":{id_b},\"result\":\"B\"}}\n"),
        );
        feed(
            &harness,
            &format!("{{\"jsonrpc\":\"2.0\",\"id\":{id_a},\"result\":\"A\"}}\n"),
        );
        assert_eq!(caller_two.join().unwrap().unwrap(), json!("B"));
        assert_eq!(caller.join().unwrap().unwrap(), json!("A"));
    }

    #[test]
    fn a_remote_error_reply_fails_the_call_with_its_error() {
        let harness = harness();
        let peer = harness.peer.clone();
        let caller = thread::spawn(move || peer.call("ask", Value::Null, WAIT));
        let request = next_message(&harness);
        let id = request["id"].clone();
        assert!(request.get("params").is_none(), "a null params is left out");
        feed(
            &harness,
            &format!(
                "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"error\":{{\"code\":-32602,\"message\":\"bad\"}}}}\n"
            ),
        );
        match caller.join().unwrap() {
            Err(Error::Remote(error)) => assert_eq!(i32::from(error.code), -32602),
            other => panic!("expected a remote error, got {other:?}"),
        }
    }

    #[test]
    fn replies_to_unknown_ids_are_ignored() {
        let harness = harness();
        feed(&harness, "{\"jsonrpc\":\"2.0\",\"id\":77,\"result\":1}\n");
        feed(
            &harness,
            "{\"jsonrpc\":\"2.0\",\"id\":\"s\",\"result\":1}\n",
        );
        feed(&harness, "{\"jsonrpc\":\"2.0\",\"method\":\"note\"}\n");
        assert_eq!(harness.notes.recv_timeout(WAIT).unwrap().0, "note");
    }

    #[test]
    fn a_call_times_out_and_a_late_reply_is_dropped() {
        let harness = harness();
        let result = harness
            .peer
            .call("slow", Value::Null, Duration::from_millis(50));
        assert!(matches!(result, Err(Error::Timeout)));
        let request = next_message(&harness);
        let id = request["id"].clone();
        feed(
            &harness,
            &format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":1}}\n"),
        );
        // The peer keeps working after the late reply.
        feed(&harness, "{\"jsonrpc\":\"2.0\",\"method\":\"after\"}\n");
        assert_eq!(harness.notes.recv_timeout(WAIT).unwrap().0, "after");
    }

    #[test]
    fn closing_fails_pending_calls_and_refuses_new_ones() {
        // The end of the stream fails pending calls, and new calls then fail at once.
        {
            let harness = harness();
            let peer = harness.peer.clone();
            let caller = thread::spawn(move || peer.call("waiting", Value::Null, WAIT));
            let _ = next_message(&harness);
            let Harness { input, peer, .. } = harness;
            drop(input);
            assert!(matches!(caller.join().unwrap(), Err(Error::Closed)));
            // Once the stream has ended, new calls fail at once.
            assert!(matches!(
                peer.call("later", Value::Null, WAIT),
                Err(Error::Closed)
            ));
            assert!(peer.is_closed());
        }

        // close() fails pending calls and refuses new notifications.
        {
            let harness = harness();
            let peer = harness.peer.clone();
            let caller = thread::spawn(move || peer.call("waiting", Value::Null, WAIT));
            let _ = next_message(&harness);
            harness.peer.close();
            assert!(matches!(caller.join().unwrap(), Err(Error::Closed)));
            assert!(matches!(
                harness.peer.notify("note", Value::Null),
                Err(Error::Closed)
            ));
        }
    }

    #[test]
    fn a_reply_that_arrives_before_the_end_of_the_stream_is_still_delivered() {
        let harness = harness();
        let peer = harness.peer.clone();
        let caller = thread::spawn(move || peer.call("quick", Value::Null, WAIT));
        let id = next_message(&harness)["id"].clone();
        // The reply and the end of the stream arrive together.
        feed(
            &harness,
            &format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":\"ok\"}}\n"),
        );
        let Harness { input, .. } = harness;
        drop(input);
        assert_eq!(caller.join().unwrap().unwrap(), json!("ok"));
    }

    #[test]
    fn notify_writes_a_message_without_an_id() {
        let harness = harness();
        harness.peer.notify("ping", json!({"n": 1})).unwrap();
        let message = next_message(&harness);
        assert_eq!(
            message,
            json!({"jsonrpc": "2.0", "method": "ping", "params": {"n": 1}})
        );
    }
}
