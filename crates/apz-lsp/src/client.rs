//! One server's connection: requests with timeouts, notifications, the diagnostics the server publishes, and the
//! server's own requests answered so it never waits on us.

use std::collections::HashMap;
use std::io::{BufReader, Read, Write};
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::LspError;
use crate::frame;
use crate::lock;
use crate::types::{Diagnostic, parse_diagnostics};

/// The server's two ends: what it writes to us and what we write to it, and the process when there is one.
pub struct Transport {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub child: Option<Child>,
    /// The tail of the server's stderr, quoted in errors when the server stops.
    pub stderr: Option<Arc<Mutex<Vec<u8>>>>,
}

enum Reply {
    Ok(Value),
    Failed(i64, String),
    Closed,
}

type Writer = Arc<Mutex<Box<dyn Write + Send>>>;
type Pending = Arc<Mutex<HashMap<i64, Sender<Reply>>>>;

#[derive(Default)]
struct Book {
    /// Bumped by every publish, so a waiter can tell a fresh answer from an old one.
    seq: u64,
    closed: bool,
    by_uri: HashMap<String, (u64, Vec<Diagnostic>)>,
}

/// One server's connection. Dropping it kills the process, if it is still running.
pub struct Client {
    name: String,
    writer: Writer,
    pending: Pending,
    book: Arc<(Mutex<Book>, Condvar)>,
    next_id: AtomicI64,
    child: Mutex<Option<Child>>,
    stderr: Option<Arc<Mutex<Vec<u8>>>>,
    /// Set by the reader when the server's output ends.
    gone: Arc<AtomicBool>,
}

impl Client {
    /// Connects to a started server. The handshake is the caller's: see [`crate::pool`].
    pub fn start(name: &str, transport: Transport) -> Result<Client, LspError> {
        let Transport {
            reader,
            writer,
            child,
            stderr,
        } = transport;
        let writer: Writer = Arc::new(Mutex::new(writer));
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let book = Arc::new((Mutex::new(Book::default()), Condvar::new()));
        let gone = Arc::new(AtomicBool::new(false));
        let (w, p, b, g) = (writer.clone(), pending.clone(), book.clone(), gone.clone());
        thread::Builder::new()
            .name(format!("lsp-{name}"))
            .spawn(move || read_loop(reader, w, p, b, g))
            .map_err(|e| LspError::Protocol(format!("{name}: cannot start its reader: {e}")))?;
        Ok(Client {
            name: name.to_string(),
            writer,
            pending,
            book,
            next_id: AtomicI64::new(1),
            child: Mutex::new(child),
            stderr,
            gone,
        })
    }

    pub fn is_closed(&self) -> bool {
        self.gone.load(Ordering::SeqCst)
    }

    /// Sends a request and waits for its answer, at most `timeout`.
    pub fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, LspError> {
        if self.is_closed() {
            return Err(self.closed());
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        lock(&self.pending).insert(id, tx);
        let sent = frame::write(
            &mut **lock(&self.writer),
            &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        );
        if sent.is_err() {
            lock(&self.pending).remove(&id);
            return Err(self.closed());
        }
        match rx.recv_timeout(timeout) {
            Ok(Reply::Ok(value)) => Ok(value),
            Ok(Reply::Failed(code, message)) => Err(LspError::Rpc {
                method: method.to_string(),
                code,
                message,
            }),
            Ok(Reply::Closed) | Err(RecvTimeoutError::Disconnected) => Err(self.closed()),
            Err(RecvTimeoutError::Timeout) => {
                lock(&self.pending).remove(&id);
                let _ = self.notify("$/cancelRequest", json!({ "id": id }));
                Err(LspError::Timeout {
                    method: method.to_string(),
                })
            }
        }
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<(), LspError> {
        frame::write(
            &mut **lock(&self.writer),
            &json!({"jsonrpc": "2.0", "method": method, "params": params}),
        )
        .map_err(|e| LspError::Closed {
            detail: format!("{}: {e}", self.name),
        })
    }

    /// The publish count so far: take it before a change, then wait with [`Client::diagnostics_after`].
    pub fn diagnostics_seq(&self) -> u64 {
        lock(&self.book.0).seq
    }

    /// The diagnostics a document got after publish `since`, waiting up to `timeout`; `None` when none came.
    pub fn diagnostics_after(
        &self,
        uri: &str,
        since: u64,
        timeout: Duration,
    ) -> Option<Vec<Diagnostic>> {
        let (mutex, cv) = &*self.book;
        let fresh = |b: &Book| b.by_uri.get(uri).is_some_and(|(seq, _)| *seq > since);
        let guard = lock(mutex);
        let (guard, _) = cv
            .wait_timeout_while(guard, timeout, |b| !b.closed && !fresh(b))
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if fresh(&guard) {
            guard.by_uri.get(uri).map(|(_, items)| items.clone())
        } else {
            None
        }
    }

    /// The diagnostics a document last got, whenever that was.
    pub fn latest_diagnostics(&self, uri: &str) -> Option<Vec<Diagnostic>> {
        lock(&self.book.0)
            .by_uri
            .get(uri)
            .map(|(_, items)| items.clone())
    }

    /// Asks the server to shut down and exit, and waits up to `grace` for it to leave.
    pub fn shutdown(&self, grace: Duration) {
        if !self.is_closed() {
            let _ = self.request("shutdown", Value::Null, grace);
            let _ = self.notify("exit", Value::Null);
        }
        let started = Instant::now();
        let mut child = lock(&self.child);
        if let Some(process) = child.as_mut() {
            while started.elapsed() < grace {
                if matches!(process.try_wait(), Ok(Some(_))) {
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
    }

    fn closed(&self) -> LspError {
        let tail = self
            .stderr
            .as_ref()
            .map(|s| String::from_utf8_lossy(&lock(s)[..]).trim().to_string())
            .unwrap_or_default();
        let detail = if tail.is_empty() {
            format!("{} stopped", self.name)
        } else {
            format!("{} stopped: {tail}", self.name)
        };
        LspError::Closed { detail }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let child = self
            .child
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(process) = child.as_mut() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
}

fn read_loop(
    reader: Box<dyn Read + Send>,
    writer: Writer,
    pending: Pending,
    book: Arc<(Mutex<Book>, Condvar)>,
    gone: Arc<AtomicBool>,
) {
    let mut reader = BufReader::new(reader);
    while let Ok(Some(msg)) = frame::read(&mut reader) {
        dispatch(msg, &writer, &pending, &book);
    }
    gone.store(true, Ordering::SeqCst);
    lock(&book.0).closed = true;
    book.1.notify_all();
    for (_, tx) in lock(&pending).drain() {
        let _ = tx.send(Reply::Closed);
    }
}

fn dispatch(msg: Value, writer: &Writer, pending: &Pending, book: &(Mutex<Book>, Condvar)) {
    match (msg.get("method").and_then(Value::as_str), msg.get("id")) {
        (Some(method), Some(id)) => {
            // The server asks us something (configuration, a progress token, a registration): answer it, empty.
            let result = match method {
                "workspace/configuration" => {
                    let items = msg["params"]["items"].as_array().map_or(0, Vec::len);
                    Value::Array(vec![Value::Null; items])
                }
                _ => Value::Null,
            };
            let _ = frame::write(
                &mut **lock(writer),
                &json!({"jsonrpc": "2.0", "id": id, "result": result}),
            );
        }
        (Some("textDocument/publishDiagnostics"), None) => {
            let params = &msg["params"];
            if let Some(uri) = params["uri"].as_str() {
                let items = parse_diagnostics(&params["diagnostics"]);
                let mut b = lock(&book.0);
                b.seq += 1;
                let seq = b.seq;
                b.by_uri.insert(uri.to_string(), (seq, items));
                drop(b);
                book.1.notify_all();
            }
        }
        (None, Some(id)) => {
            if let Some(id) = id.as_i64()
                && let Some(tx) = lock(pending).remove(&id)
            {
                let reply = match msg.get("error") {
                    Some(e) => Reply::Failed(
                        e["code"].as_i64().unwrap_or(0),
                        e["message"].as_str().unwrap_or("").to_string(),
                    ),
                    None => Reply::Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                };
                let _ = tx.send(reply);
            }
        }
        _ => {}
    }
}
