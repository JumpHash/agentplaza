//! The warm server (PLAN 3.8): a language server started in a base, kept running through the base's snapshot, and reached
//! by the harness's tools over a Unix socket instead of a server of their own.
//!
//! The base runs [`serve_installed`] as a background command. It starts the server, does the handshake once, and listens
//! on a socket. Each connection is one client: the harness's pool, or the warm-up [`warm`] runs in the base. A client's
//! `initialize` is answered from the handshake's result and its `initialized` is dropped; its `shutdown` and `exit` never
//! reach the server, so a client that goes away does not stop the warm server. Its other messages pass to the server, and
//! the server's messages to the connected client. With no client connected, the server's own requests are answered `null`
//! so that it never waits on us, and its notifications are dropped. One client is connected at a time: a newer connection
//! takes over from an older one.
//!
//! A client's `didOpen` for a document the server already has open becomes a change of its whole text: an earlier client
//! opened it, and the server keeps its state across clients.

use std::path::{Path, PathBuf};

/// Where a base's warm servers listen, in the VM: `<WARM_DIR>/<server id>.sock` (the harness's workspace keeps its state
/// under `.plaza`, which the model's tools never reach).
pub const WARM_DIR: &str = "/workspace/.plaza/lsp";

/// The socket a server id listens on in `dir`.
pub fn socket_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.sock"))
}

#[cfg(unix)]
pub use sockets::{connect, serve, serve_installed, warm};

/// The transport to a warm server: none without Unix sockets (Windows), so the pool starts its own server there.
#[cfg(not(unix))]
pub fn connect(_socket: &Path) -> Option<crate::client::Transport> {
    None
}

/// The warm server's socket side. The bridge runs in a base's VM (Linux); hubs and nodes on any OS only name its
/// socket paths ([`socket_path`], [`WARM_DIR`]).
#[cfg(unix)]
mod sockets {
    use super::*;
    use std::collections::HashSet;
    use std::fs;
    use std::io::{self, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::process::Child;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use serde_json::{Value, json};

    use crate::client::{Client, Transport};
    use crate::servers::{Language, ServerSpec};
    use crate::types::path_to_uri;
    use crate::{LspError, Timeouts, frame, lock, pool};

    /// The id the bridge's own handshake uses. No client's id is negative.
    const HANDSHAKE_ID: i64 = -1;

    /// How often a warm-up retries a connection the bridge has not yet made.
    const CONNECT_RETRY: Duration = Duration::from_millis(200);

    /// The transport to the warm server on `socket`, when one is listening there.
    pub fn connect(socket: &Path) -> Option<Transport> {
        let stream = UnixStream::connect(socket).ok()?;
        let reader = stream.try_clone().ok()?;
        Some(Transport {
            reader: Box::new(reader),
            writer: Box::new(stream),
            child: None,
            stderr: None,
        })
    }

    /// Starts the installed server of `spec` in `root` and serves it on `socket` (see [`serve`]).
    pub fn serve_installed(
        socket: &Path,
        spec: &ServerSpec,
        root: &Path,
        start: Duration,
    ) -> Result<(), LspError> {
        let transport = pool::process(spec, root)?;
        serve(socket, transport, spec, root, start)
    }

    /// Does the handshake with the server behind `transport` (within `start`), then serves it on `socket` until the server's
    /// output ends. The socket is removed when it returns.
    pub fn serve(
        socket: &Path,
        transport: Transport,
        spec: &ServerSpec,
        root: &Path,
        start: Duration,
    ) -> Result<(), LspError> {
        let Transport {
            reader,
            mut writer,
            child,
            stderr: _,
        } = transport;
        // The server's messages come through a thread of their own, so the handshake can time out.
        let (tx, rx) = mpsc::channel::<Value>();
        thread::Builder::new()
            .name(format!("lsp-bridge-{}", spec.id))
            .spawn(move || {
                let mut reader = BufReader::new(reader);
                while let Ok(Some(msg)) = frame::read(&mut reader) {
                    if tx.send(msg).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| {
                LspError::Protocol(format!("{}: cannot start the bridge: {e}", spec.id))
            })?;

        send(
            &mut writer,
            &json!({"jsonrpc": "2.0", "id": HANDSHAKE_ID, "method": "initialize", "params": pool::initialize_params(root, spec)}),
        )?;
        let deadline = Instant::now() + start;
        let init = loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(msg) if msg.get("method").is_none() && msg["id"] == HANDSHAKE_ID => {
                    if let Some(error) = msg.get("error") {
                        return Err(LspError::Rpc {
                            method: "initialize".into(),
                            code: error["code"].as_i64().unwrap_or(0),
                            message: error["message"].as_str().unwrap_or("").into(),
                        });
                    }
                    break msg["result"].clone();
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {
                    return Err(LspError::Timeout {
                        method: "initialize".into(),
                    });
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(LspError::Closed {
                        detail: format!("{} stopped during its handshake", spec.command),
                    });
                }
            }
        };
        send(
            &mut writer,
            &json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        )?;

        let shared = Arc::new(Shared {
            server: Mutex::new(writer),
            init,
            open: Mutex::new(HashSet::new()),
            client: Mutex::new(None),
            next: AtomicU64::new(1),
            _child: Mutex::new(child),
        });
        let (done_tx, done_rx) = mpsc::channel::<()>();
        let forward = shared.clone();
        thread::spawn(move || {
            for msg in rx {
                forward.relay_server(msg);
            }
            let _ = done_tx.send(());
        });

        if let Some(dir) = socket.parent() {
            fs::create_dir_all(dir).map_err(|e| io_error(socket, e))?;
        }
        let _ = fs::remove_file(socket);
        let listener = UnixListener::bind(socket).map_err(|e| io_error(socket, e))?;
        let accept = shared.clone();
        thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                // Numbered in accept order, so the newest connection is the one the server's messages go to.
                let number = accept.next.fetch_add(1, Ordering::SeqCst);
                let session = accept.clone();
                thread::spawn(move || session.session(number, conn));
            }
        });

        let _ = done_rx.recv();
        let _ = fs::remove_file(socket);
        Err(LspError::Closed {
            detail: format!("the warm {} stopped", spec.id),
        })
    }

    /// The warm-up (PLAN 3.8), run in the base after the server has started: connects to the warm server on `socket` (retrying
    /// for `wait`), opens `file` when one is given, and asks the server for its workspace symbols until it has some. Returns
    /// whether it did, which is the server having indexed. The server stays up: only this client's connection closes.
    pub fn warm(
        socket: &Path,
        file: Option<&Path>,
        wait: Duration,
        timeouts: Timeouts,
    ) -> Result<bool, LspError> {
        let deadline = Instant::now() + wait;
        let transport = loop {
            match connect(socket) {
                Some(transport) => break transport,
                None if Instant::now() < deadline => thread::sleep(CONNECT_RETRY),
                None => {
                    return Err(LspError::Closed {
                        detail: format!("no warm server listens at {}", socket.display()),
                    });
                }
            }
        };
        let client = Client::start("warm", transport)?;
        client.request(
            "initialize",
            json!({ "processId": null, "rootUri": null, "capabilities": {} }),
            timeouts.start,
        )?;
        client.notify("initialized", json!({}))?;
        if let Some(file) = file
            && let Some(language) = Language::from_path(&file.to_string_lossy())
            && let Ok(text) = fs::read_to_string(file)
        {
            client.notify(
                "textDocument/didOpen",
                json!({"textDocument": {
                    "uri": path_to_uri(file),
                    "languageId": language.language_id(&file.to_string_lossy()),
                    "version": 1,
                    "text": text,
                }}),
            )?;
        }
        loop {
            match client.request("workspace/symbol", json!({ "query": "" }), timeouts.request) {
                Ok(answer) if answer.as_array().is_some_and(|symbols| !symbols.is_empty()) => {
                    return Ok(true);
                }
                // Still indexing (empty), or not yet answering: asked again until the deadline.
                Ok(_) | Err(LspError::Timeout { .. } | LspError::Rpc { .. }) => {}
                Err(other) => return Err(other),
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            thread::sleep(timeouts.warm_retry);
        }
    }

    /// What the bridge shares between its threads.
    struct Shared {
        /// The server's input.
        server: Mutex<Box<dyn Write + Send>>,
        /// The handshake's result, given to each client's `initialize`.
        init: Value,
        /// The documents the server has open.
        open: Mutex<HashSet<String>>,
        /// The connected client, by its number, with its writer.
        client: Mutex<Option<(u64, Arc<Mutex<UnixStream>>)>>,
        /// The number the next connection gets.
        next: AtomicU64,
        /// The server's process, kept for the bridge's life (the server ends with its own output, not with a drop).
        _child: Mutex<Option<Child>>,
    }

    impl Shared {
        /// A message from the server: to the connected client; a server's request with no client is answered null.
        fn relay_server(&self, msg: Value) {
            let client = lock(&self.client).clone();
            match client {
                Some((_, conn)) => {
                    let _ = send(&mut *lock(&conn), &msg);
                }
                None if msg.get("method").is_some() && msg.get("id").is_some() => {
                    self.to_server(&json!({"jsonrpc": "2.0", "id": msg["id"], "result": null}));
                }
                None => {}
            }
        }

        /// One client's connection, numbered `number` in accept order: its messages until it closes. The newest connection
        /// takes the server's messages; when it closes, the connection before it does not take them back.
        fn session(&self, number: u64, conn: UnixStream) {
            let Ok(reader_side) = conn.try_clone() else {
                return;
            };
            let writer = Arc::new(Mutex::new(conn));
            {
                let mut current = lock(&self.client);
                if current.as_ref().is_none_or(|(n, _)| *n < number) {
                    *current = Some((number, writer.clone()));
                }
            }
            let mut reader = BufReader::new(reader_side);
            while let Ok(Some(msg)) = frame::read(&mut reader) {
                self.relay_client(msg, &writer);
            }
            let mut current = lock(&self.client);
            if current.as_ref().is_some_and(|(n, _)| *n == number) {
                *current = None;
            }
        }

        /// One message from a client: the handshake is answered here, and the rest goes to the server.
        fn relay_client(&self, msg: Value, writer: &Mutex<UnixStream>) {
            let method = msg["method"].as_str().unwrap_or("").to_string();
            let id = msg.get("id").cloned();
            match (method.as_str(), id) {
                ("initialize", Some(id)) => {
                    let _ = send(
                        &mut *lock(writer),
                        &json!({"jsonrpc": "2.0", "id": id, "result": self.init}),
                    );
                }
                ("shutdown", Some(id)) => {
                    let _ = send(
                        &mut *lock(writer),
                        &json!({"jsonrpc": "2.0", "id": id, "result": null}),
                    );
                }
                ("initialized" | "exit", _) => {}
                ("textDocument/didOpen", _) => {
                    let doc = &msg["params"]["textDocument"];
                    let uri = doc["uri"].as_str().unwrap_or("").to_string();
                    if lock(&self.open).insert(uri.clone()) {
                        self.to_server(&msg);
                    } else {
                        self.to_server(&json!({
                            "jsonrpc": "2.0",
                            "method": "textDocument/didChange",
                            "params": {
                                "textDocument": {"uri": uri, "version": doc["version"]},
                                "contentChanges": [{"text": doc["text"]}],
                            },
                        }));
                    }
                }
                ("textDocument/didClose", _) => {
                    let uri = msg["params"]["textDocument"]["uri"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    lock(&self.open).remove(&uri);
                    self.to_server(&msg);
                }
                _ => self.to_server(&msg),
            }
        }

        fn to_server(&self, msg: &Value) {
            let _ = send(&mut *lock(&self.server), msg);
        }
    }

    fn send(w: &mut (impl Write + ?Sized), msg: &Value) -> Result<(), LspError> {
        frame::write(w, msg).map_err(|e| LspError::Protocol(format!("the warm server's pipe: {e}")))
    }

    fn io_error(socket: &Path, e: io::Error) -> LspError {
        LspError::Protocol(format!("cannot listen on {}: {e}", socket.display()))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::fake::{self, Script};
        use crate::servers::RUST_ANALYZER;
        use crate::types::{Location, Position, Range};

        /// A socket in a fresh directory of its own, short enough for the OS's limit.
        fn socket_in(name: &str) -> PathBuf {
            let dir = std::env::temp_dir().join(format!("apz-br-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            dir.join("rust-analyzer.sock")
        }

        fn location(path: &str) -> Location {
            Location {
                uri: path_to_uri(Path::new(path)),
                range: Range {
                    start: Position {
                        line: 1,
                        character: 4,
                    },
                    end: Position {
                        line: 1,
                        character: 9,
                    },
                },
            }
        }

        /// Serves a fake server with `script` on a fresh socket, on a thread, and waits until the socket listens.
        fn warm_server(name: &str, script: Script) -> (PathBuf, fake::Seen) {
            let socket = socket_in(name);
            let (transport, seen) = fake::transport(script);
            let path = socket.clone();
            thread::spawn(move || {
                let _ = serve(
                    &path,
                    transport,
                    &RUST_ANALYZER,
                    Path::new("/"),
                    Duration::from_secs(5),
                );
            });
            let deadline = Instant::now() + Duration::from_secs(5);
            while UnixStream::connect(&socket).is_err() {
                assert!(Instant::now() < deadline, "the bridge never listened");
                thread::sleep(Duration::from_millis(10));
            }
            (socket, seen)
        }

        #[test]
        fn a_client_is_answered_from_the_warm_servers_handshake_and_never_stops_it() {
            let script = Script {
                definition: vec![location("/ws/a.rs")],
                ..Script::default()
            };
            let (socket, seen) = warm_server("handshake", script);
            let transport = connect(&socket).unwrap();
            let client = Client::start("rust-analyzer", transport).unwrap();
            let init = client
                .request(
                    "initialize",
                    json!({"capabilities": {}}),
                    Duration::from_secs(5),
                )
                .unwrap();
            assert_eq!(init["capabilities"]["definitionProvider"], true);
            client.notify("initialized", json!({})).unwrap();
            client
            .notify(
                "textDocument/didOpen",
                json!({"textDocument": {"uri": "file:///ws/a.rs", "languageId": "rust", "version": 1, "text": "x"}}),
            )
            .unwrap();
            // The client goes away with the usual shutdown: the server keeps running for the next one.
            client.shutdown(Duration::from_millis(200));
            drop(client);

            // A second client finds the server up and initialized, and its open of the same document is a change.
            let again = Client::start("rust-analyzer", connect(&socket).unwrap()).unwrap();
            again
                .request("initialize", json!({}), Duration::from_secs(5))
                .unwrap();
            again
            .notify(
                "textDocument/didOpen",
                json!({"textDocument": {"uri": "file:///ws/a.rs", "languageId": "rust", "version": 1, "text": "y"}}),
            )
            .unwrap();
            let answer = again
            .request(
                "textDocument/definition",
                json!({"textDocument": {"uri": "file:///ws/a.rs"}, "position": {"line": 1, "character": 4}}),
                Duration::from_secs(5),
            )
            .unwrap();
            assert_eq!(answer[0]["uri"], "file:///ws/a.rs");
            let methods = lock(&seen.methods).clone();
            assert!(
                !methods.iter().any(|m| m == "shutdown" || m == "exit"),
                "{methods:?}"
            );
            assert_eq!(methods.iter().filter(|m| *m == "initialize").count(), 1);
            assert_eq!(lock(&seen.opened).len(), 1);
            assert_eq!(lock(&seen.changed).len(), 1);
        }

        #[test]
        fn warm_waits_until_the_server_has_indexed_and_then_reports_it() {
            let script = Script {
                indexed_after: 2,
                ..Script::default()
            };
            let (socket, _seen) = warm_server("indexing", script);
            let indexed = warm(
                &socket,
                None,
                Duration::from_secs(10),
                Timeouts {
                    warm_retry: Duration::from_millis(10),
                    ..Timeouts::default()
                },
            )
            .unwrap();
            assert!(indexed);
        }

        #[test]
        fn warm_reports_an_unindexed_server_at_its_deadline_and_fails_without_a_socket() {
            let script = Script {
                indexed_after: u32::MAX,
                ..Script::default()
            };
            let (socket, _seen) = warm_server("never", script);
            let quick = Timeouts {
                warm_retry: Duration::from_millis(5),
                ..Timeouts::default()
            };
            assert!(!warm(&socket, None, Duration::from_millis(300), quick).unwrap());
            let missing = socket_in("missing");
            let err = warm(&missing, None, Duration::from_millis(50), quick).unwrap_err();
            assert!(err.to_string().contains("no warm server"), "{err}");
        }

        #[test]
        fn a_pool_with_a_warm_server_reaches_it_without_launching_one() {
            let script = Script {
                definition: vec![location("/ws/b.rs")],
                indexed_after: 0,
                ..Script::default()
            };
            let (socket, _seen) = warm_server("pool", script);
            let dir = socket.parent().unwrap().to_path_buf();
            let root = std::env::temp_dir().join(format!("apz-br-root-{}", std::process::id()));
            fs::create_dir_all(&root).unwrap();
            let file = root.join("a.rs");
            fs::write(&file, "fn a() {}\n").unwrap();
            let launched = Arc::new(Mutex::new(0u32));
            let counter = launched.clone();
            let pool = pool::Pool::with_launch(Timeouts::default(), move |_, _| {
                *lock(&counter) += 1;
                Err(LspError::Protocol("no launch expected".into()))
            })
            .with_warm(dir, &root);
            let found = pool
                .definition(
                    &root,
                    &file,
                    Position {
                        line: 0,
                        character: 0,
                    },
                )
                .unwrap();
            assert_eq!(found, vec![location("/ws/b.rs")]);
            assert_eq!(
                *lock(&launched),
                0,
                "the warm server was reached, not launched"
            );
            assert_eq!(pool.warmed(), 1);
            // A root other than the warm one is not reached: the pool starts its own server there.
            let other = pool::Pool::with_launch(Timeouts::default(), |_, _| {
                Err(LspError::Protocol("launched".into()))
            })
            .with_warm(socket.parent().unwrap(), "/elsewhere");
            assert!(
                other
                    .definition(
                        &root,
                        &file,
                        Position {
                            line: 0,
                            character: 0
                        }
                    )
                    .is_err()
            );
            assert_eq!(other.warmed(), 0);
        }
    }
}
