//! A scripted language server in this process, for tests: the protocol and the pool, without a real server.
//!
//! [`transport`] returns the client's two ends of a connection whose other end is a thread that answers what the
//! [`Script`] says and records what it was asked. Nothing is spawned, so the same tests run on every OS.

use std::io::{BufReader, PipeReader, PipeWriter};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{Value, json};

use crate::client::Transport;
use crate::frame;
use crate::lock;
use crate::types::{
    Diagnostic, DocumentSymbol, Location, diagnostic_json, document_symbol_json, location_json,
};

/// What the fake answers.
#[derive(Clone, Debug, Default)]
pub struct Script {
    /// The answer to `textDocument/definition`.
    pub definition: Vec<Location>,
    /// The answer to `textDocument/references`.
    pub references: Vec<Location>,
    /// The answer to `textDocument/documentSymbol`.
    pub symbols: Vec<DocumentSymbol>,
    /// Published for every document opened or changed.
    pub diagnostics: Vec<Diagnostic>,
    /// When set, every request after the handshake is refused with this message.
    pub refuse: Option<String>,
    /// The first this many definition and reference answers are empty, as a server still indexing answers.
    pub empty_answers: u32,
    /// The first this many `workspace/symbol` answers are empty: a server answers its symbols once it has indexed.
    pub indexed_after: u32,
}

/// What the fake was asked, in order.
#[derive(Clone, Debug, Default)]
pub struct Seen {
    pub methods: Arc<Mutex<Vec<String>>>,
    /// The `initializationOptions` the client sent.
    pub init_options: Arc<Mutex<Option<Value>>>,
    /// The URIs of the documents opened.
    pub opened: Arc<Mutex<Vec<String>>>,
    /// The URIs of the documents changed (each sent whole).
    pub changed: Arc<Mutex<Vec<String>>>,
}

/// A transport whose far end answers `script` on its own thread, and what it was asked.
pub fn transport(script: Script) -> (Transport, Seen) {
    let (server_in, to_server) = std::io::pipe().expect("a pipe");
    let (from_server, server_out) = std::io::pipe().expect("a pipe");
    let seen = Seen::default();
    let record = seen.clone();
    thread::spawn(move || serve(BufReader::new(server_in), server_out, script, record));
    (
        Transport {
            reader: Box::new(from_server),
            writer: Box::new(to_server),
            child: None,
            stderr: None,
        },
        seen,
    )
}

fn serve(mut input: BufReader<PipeReader>, mut output: PipeWriter, script: Script, seen: Seen) {
    let mut empty_left = script.empty_answers;
    let mut unindexed = script.indexed_after;
    while let Ok(Some(msg)) = frame::read(&mut input) {
        let method = msg["method"].as_str().unwrap_or("").to_string();
        lock(&seen.methods).push(method.clone());
        if method == "exit" {
            break;
        }
        let Some(id) = msg.get("id").cloned() else {
            // A notification: a document opened or changed gets the scripted diagnostics.
            let uri = msg["params"]["textDocument"]["uri"]
                .as_str()
                .unwrap_or("")
                .to_string();
            let publish = match method.as_str() {
                "textDocument/didOpen" => {
                    lock(&seen.opened).push(uri.clone());
                    true
                }
                "textDocument/didChange" => {
                    lock(&seen.changed).push(uri.clone());
                    true
                }
                _ => false,
            };
            if publish {
                let items: Vec<Value> = script.diagnostics.iter().map(diagnostic_json).collect();
                let note = json!({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {"uri": uri, "diagnostics": items}});
                if frame::write(&mut output, &note).is_err() {
                    break;
                }
            }
            continue;
        };
        if method == "initialize" {
            *lock(&seen.init_options) = Some(msg["params"]["initializationOptions"].clone());
        }
        let refused = script
            .refuse
            .as_deref()
            .filter(|_| !matches!(method.as_str(), "initialize" | "shutdown"));
        let reply = match (refused, method.as_str()) {
            (Some(message), _) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603, "message": message}})
            }
            (None, "initialize") => json!({"jsonrpc": "2.0", "id": id, "result": {"capabilities": {
                "textDocumentSync": 1,
                "definitionProvider": true,
                "referencesProvider": true,
                "documentSymbolProvider": true
            }}}),
            (None, "shutdown") => json!({"jsonrpc": "2.0", "id": id, "result": null}),
            (None, "textDocument/definition" | "textDocument/references") => {
                let scripted = if method == "textDocument/definition" {
                    &script.definition
                } else {
                    &script.references
                };
                let items: Vec<Value> = if empty_left > 0 {
                    empty_left -= 1;
                    Vec::new()
                } else {
                    scripted.iter().map(location_json).collect()
                };
                json!({"jsonrpc": "2.0", "id": id, "result": items})
            }
            (None, "workspace/symbol") => {
                let items: Vec<Value> = if unindexed > 0 {
                    unindexed -= 1;
                    Vec::new()
                } else {
                    vec![json!({"name": "indexed", "kind": 12, "location": {
                        "uri": "file:///ws/a.rs",
                        "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}
                    }})]
                };
                json!({"jsonrpc": "2.0", "id": id, "result": items})
            }
            (None, "textDocument/documentSymbol") => {
                let items: Vec<Value> = script.symbols.iter().map(document_symbol_json).collect();
                json!({"jsonrpc": "2.0", "id": id, "result": items})
            }
            _ => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not found"}})
            }
        };
        if frame::write(&mut output, &reply).is_err() {
            break;
        }
    }
}
