//! The pool: one server per (server, workspace root), started on the first query for its language, kept up, and shut
//! down when the pool drops. A server that stopped is started again by the next query.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use serde_json::{Value, json};

use crate::client::{Client, Transport};
use crate::limits;
use crate::lock;
use crate::servers::{self, Language, ServerSpec};
use crate::types::{
    Diagnostic, DocumentSymbol, Location, Position, parse_document_symbols, parse_locations,
    path_to_uri, position_json,
};
use crate::{LspError, Timeouts};

/// The stderr kept for an error message: its last bytes.
const STDERR_TAIL: usize = 8 * 1024;

type Launch = dyn Fn(&ServerSpec, &Path) -> Result<Transport, LspError> + Send + Sync;

/// Starts and keeps the language servers.
pub struct Pool {
    timeouts: Timeouts,
    launch: Box<Launch>,
    servers: Mutex<HashMap<(&'static str, PathBuf), Arc<Server>>>,
    starts: AtomicU64,
    /// Where the warm servers listen (PLAN 3.8), and the workspace root they were started in: a server for that root is
    /// reached on its socket instead of launched.
    warm: Option<(PathBuf, PathBuf)>,
    warmed: AtomicU64,
}

/// A started, initialized server, and the documents it has open.
pub struct Server {
    client: Client,
    docs: Mutex<HashMap<String, Doc>>,
    started: Instant,
}

struct Doc {
    version: i32,
    hash: u64,
}

/// A file's diagnostics. `settled` is false when the server did not publish for the file in time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    pub items: Vec<Diagnostic>,
    pub settled: bool,
}

impl Pool {
    /// A pool that starts the servers installed on this machine.
    pub fn new(timeouts: Timeouts) -> Pool {
        Pool::with_launch(timeouts, process)
    }

    /// A pool that starts its servers some other way: tests start scripted ones.
    pub fn with_launch(
        timeouts: Timeouts,
        launch: impl Fn(&ServerSpec, &Path) -> Result<Transport, LspError> + Send + Sync + 'static,
    ) -> Pool {
        Pool {
            timeouts,
            launch: Box::new(launch),
            servers: Mutex::new(HashMap::new()),
            starts: AtomicU64::new(0),
            warm: None,
            warmed: AtomicU64::new(0),
        }
    }

    /// Reaches the warm servers (`dir` holds their sockets, see [`crate::bridge`]) for the workspace `root`: a server
    /// for `root` is taken from its socket when one listens there, and only launched when none does.
    pub fn with_warm(mut self, dir: impl Into<PathBuf>, root: impl Into<PathBuf>) -> Pool {
        self.warm = Some((dir.into(), root.into()));
        self
    }

    /// How many servers were reached on their warm sockets rather than launched.
    pub fn warmed(&self) -> u64 {
        self.warmed.load(Ordering::SeqCst)
    }

    /// How many servers have been started since the pool was made.
    pub fn starts(&self) -> u64 {
        self.starts.load(Ordering::SeqCst)
    }

    /// The servers running now, as (server id, workspace root).
    pub fn running(&self) -> Vec<(&'static str, PathBuf)> {
        let mut keys: Vec<_> = lock(&self.servers).keys().cloned().collect();
        keys.sort();
        keys
    }

    /// Where the definition of the symbol at `at` (zero-based position) in `file` is.
    pub fn definition(
        &self,
        root: &Path,
        file: &Path,
        at: Position,
    ) -> Result<Vec<Location>, LspError> {
        let (server, uri) = self.open(root, file)?;
        let v = self.ask(
            &server,
            "textDocument/definition",
            position_params(&uri, at),
        )?;
        Ok(parse_locations(&v))
    }

    /// Where the symbol at `at` in `file` is used, the definition included.
    pub fn references(
        &self,
        root: &Path,
        file: &Path,
        at: Position,
    ) -> Result<Vec<Location>, LspError> {
        let (server, uri) = self.open(root, file)?;
        let mut params = position_params(&uri, at);
        params["context"] = json!({ "includeDeclaration": true });
        let v = self.ask(&server, "textDocument/references", params)?;
        Ok(parse_locations(&v))
    }

    /// The symbols of `file`, nested as the server nests them.
    pub fn document_symbols(
        &self,
        root: &Path,
        file: &Path,
    ) -> Result<Vec<DocumentSymbol>, LspError> {
        let (server, uri) = self.open(root, file)?;
        let v = server.client.request(
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": uri } }),
            self.timeouts.request,
        )?;
        Ok(parse_document_symbols(&v))
    }

    /// The diagnostics of `file` as the server stands on its current text.
    pub fn diagnostics(&self, root: &Path, file: &Path) -> Result<Checked, LspError> {
        let language = language_of(file)?;
        let server = self.server(root, language)?;
        let since = server.client.diagnostics_seq();
        let (uri, changed) = server.sync(file, language)?;
        if !changed && let Some(items) = server.client.latest_diagnostics(&uri) {
            return Ok(Checked {
                items,
                settled: true,
            });
        }
        match server
            .client
            .diagnostics_after(&uri, since, self.timeouts.diagnostics)
        {
            Some(items) => Ok(Checked {
                items,
                settled: true,
            }),
            None => Ok(Checked {
                items: server.client.latest_diagnostics(&uri).unwrap_or_default(),
                settled: false,
            }),
        }
    }

    /// A question whose answer is a symbol's definition or uses. While the server is warming up it may answer empty
    /// (it is still indexing), so an empty answer is asked again every `warm_retry`, until `warm_up` has passed.
    fn ask(&self, server: &Server, method: &str, params: Value) -> Result<Value, LspError> {
        loop {
            let answer = server
                .client
                .request(method, params.clone(), self.timeouts.request)?;
            let empty = answer.is_null() || answer.as_array().is_some_and(Vec::is_empty);
            if !empty || server.started.elapsed() >= self.timeouts.warm_up {
                return Ok(answer);
            }
            thread::sleep(self.timeouts.warm_retry);
        }
    }

    fn open(&self, root: &Path, file: &Path) -> Result<(Arc<Server>, String), LspError> {
        let language = language_of(file)?;
        let server = self.server(root, language)?;
        let (uri, _) = server.sync(file, language)?;
        Ok((server, uri))
    }

    /// The running server for the file's language under `root`, starting it when there is none.
    fn server(&self, root: &Path, language: Language) -> Result<Arc<Server>, LspError> {
        if !root.is_absolute() {
            return Err(LspError::File(format!(
                "the workspace {} is not an absolute path",
                root.display()
            )));
        }
        let spec = language.server();
        let key = (spec.id, root.to_path_buf());
        let mut map = lock(&self.servers);
        if let Some(existing) = map.get(&key)
            && !existing.client.is_closed()
        {
            return Ok(existing.clone());
        }
        let warm = self
            .warm
            .as_ref()
            .filter(|(_, warm_root)| warm_root.as_path() == root)
            .and_then(|(dir, _)| crate::bridge::connect(&crate::bridge::socket_path(dir, spec.id)));
        let transport = match warm {
            Some(transport) => {
                self.warmed.fetch_add(1, Ordering::SeqCst);
                transport
            }
            None => (self.launch)(spec, root).map_err(|e| match e {
                LspError::NoServer { command, .. } => LspError::NoServer {
                    language: language.name().to_string(),
                    command,
                },
                other => other,
            })?,
        };
        let client = Client::start(spec.id, transport)?;
        client.request(
            "initialize",
            initialize_params(root, spec),
            self.timeouts.start,
        )?;
        client.notify("initialized", json!({}))?;
        let server = Arc::new(Server {
            client,
            docs: Mutex::new(HashMap::new()),
            started: Instant::now(),
        });
        self.starts.fetch_add(1, Ordering::SeqCst);
        map.insert(key, server.clone());
        Ok(server)
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        let servers: Vec<Arc<Server>> = lock(&self.servers).drain().map(|(_, s)| s).collect();
        for server in servers {
            server.client.shutdown(self.timeouts.shutdown);
        }
    }
}

impl Server {
    /// Brings the server's copy of `file` up to the disk: opens it, or sends the whole text when it changed.
    fn sync(&self, file: &Path, language: Language) -> Result<(String, bool), LspError> {
        let text = std::fs::read_to_string(file)
            .map_err(|e| LspError::File(format!("cannot read {} as text: {e}", file.display())))?;
        let uri = path_to_uri(file);
        let hash = hash_of(&text);
        let known = lock(&self.docs)
            .get(&uri)
            .map(|doc| (doc.version, doc.hash));
        match known {
            Some((_, seen)) if seen == hash => Ok((uri, false)),
            Some((version, _)) => {
                let version = version + 1;
                self.client.notify(
                    "textDocument/didChange",
                    json!({
                        "textDocument": { "uri": uri, "version": version },
                        "contentChanges": [{ "text": text }],
                    }),
                )?;
                lock(&self.docs).insert(uri.clone(), Doc { version, hash });
                Ok((uri, true))
            }
            None => {
                let path = file.to_string_lossy();
                self.client.notify(
                    "textDocument/didOpen",
                    json!({
                        "textDocument": {
                            "uri": uri,
                            "languageId": language.language_id(&path),
                            "version": 1,
                            "text": text,
                        },
                    }),
                )?;
                lock(&self.docs).insert(uri.clone(), Doc { version: 1, hash });
                Ok((uri, true))
            }
        }
    }
}

fn language_of(file: &Path) -> Result<Language, LspError> {
    if !file.is_absolute() {
        return Err(LspError::File(format!(
            "{} is not an absolute path",
            file.display()
        )));
    }
    Language::from_path(&file.to_string_lossy())
        .ok_or_else(|| LspError::File(format!("no language server knows {}", file.display())))
}

fn position_params(uri: &str, at: Position) -> Value {
    json!({ "textDocument": { "uri": uri }, "position": position_json(at) })
}

pub(crate) fn initialize_params(root: &Path, spec: &ServerSpec) -> Value {
    let name = root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace");
    json!({
        "processId": std::process::id(),
        "rootUri": path_to_uri(root),
        "workspaceFolders": [{ "uri": path_to_uri(root), "name": name }],
        "capabilities": {
            "workspace": { "workspaceFolders": true, "configuration": false },
            "textDocument": {
                "synchronization": { "dynamicRegistration": false, "didSave": false },
                "definition": { "dynamicRegistration": false, "linkSupport": false },
                "references": { "dynamicRegistration": false },
                "documentSymbol": { "dynamicRegistration": false, "hierarchicalDocumentSymbolSupport": true },
                "publishDiagnostics": { "relatedInformation": false }
            },
            "window": { "workDoneProgress": false }
        },
        "initializationOptions": serde_json::from_str::<Value>(spec.init_options).unwrap_or(Value::Null),
        "trace": "off",
    })
}

fn hash_of(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// Starts the installed server as a child process in `root`, with its limits, and keeps its stderr's tail.
pub(crate) fn process(spec: &ServerSpec, root: &Path) -> Result<Transport, LspError> {
    let exe = servers::which(spec.command).ok_or_else(|| LspError::NoServer {
        language: String::new(),
        command: spec.command.to_string(),
    })?;
    let mut cmd = Command::new(exe);
    cmd.args(spec.args)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    limits::apply(&mut cmd, spec.limits);
    let mut child = cmd
        .spawn()
        .map_err(|e| LspError::Protocol(format!("cannot start {}: {e}", spec.command)))?;
    let tail = Arc::new(Mutex::new(Vec::new()));
    if let Some(mut err) = child.stderr.take() {
        let keep = tail.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = err.read(&mut buf)
                && n > 0
            {
                let mut t = lock(&keep);
                t.extend_from_slice(&buf[..n]);
                let excess = t.len().saturating_sub(STDERR_TAIL);
                t.drain(..excess);
            }
        });
    }
    let writer = child
        .stdin
        .take()
        .ok_or_else(|| LspError::Protocol(format!("{} has no stdin", spec.command)))?;
    let reader = child
        .stdout
        .take()
        .ok_or_else(|| LspError::Protocol(format!("{} has no stdout", spec.command)))?;
    Ok(Transport {
        reader: Box::new(reader),
        writer: Box::new(writer),
        child: Some(child),
        stderr: Some(tail),
    })
}
