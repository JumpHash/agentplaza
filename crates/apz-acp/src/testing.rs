//! A scriptable ACP client for tests. It speaks ACP over any byte stream, usually the stdio of the
//! `apz-acp-fake-agent` binary, records what the agent sends, and answers the agent's requests from a fixed
//! configuration: a permission choice, an in-memory file system, and a canned terminal.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::rpc::{self, Peer};
use crate::types::{
    self, AGENT_METHOD_NAMES, CLIENT_METHOD_NAMES, CreateTerminalRequest, InitializeResponse,
    NewSessionResponse, PermissionOptionKind, PromptResponse, ReadTextFileRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionId, SessionNotification, StopReason,
    WaitForTerminalExitRequest, WriteTextFileRequest,
};

/// How long any one call waits before the test fails.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// How the client answers `session/request_permission`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionAnswer {
    /// Selects the first option the agent offered with this kind, or cancels if it offered none.
    Kind(PermissionOptionKind),
    /// Answers `cancelled`.
    Cancelled,
}

/// The answers a [`FakeClient`] gives to what the agent asks.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub permission: PermissionAnswer,
    /// Files the agent can read and write, keyed by the path it names. Reads return the whole file; the
    /// `line` and `limit` fields are ignored.
    pub files: HashMap<String, String>,
    /// Output the stub terminal reports.
    pub terminal_output: String,
    /// Exit code the stub terminal reports.
    pub terminal_exit_code: u32,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            permission: PermissionAnswer::Kind(PermissionOptionKind::AllowOnce),
            files: HashMap::new(),
            terminal_output: String::new(),
            terminal_exit_code: 0,
        }
    }
}

/// An ACP client driving one agent. Clones share the connection and what it has recorded.
#[derive(Clone)]
pub struct FakeClient {
    peer: Peer,
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    files: Mutex<HashMap<String, String>>,
    updates: Mutex<Vec<SessionNotification>>,
    requests: Mutex<Vec<(String, Value)>>,
}

impl FakeClient {
    /// Starts a client over `reader` and `writer`. Nothing is sent until a method is called.
    pub fn spawn(
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        config: ClientConfig,
    ) -> Self {
        let shared = Arc::new(Shared {
            files: Mutex::new(config.files.clone()),
            ..Shared::default()
        });
        let for_requests = Arc::clone(&shared);
        let for_updates = Arc::clone(&shared);
        let peer = Peer::new(
            reader,
            writer,
            move |_peer, method, params| answer(&for_requests, &config, method, params),
            move |_peer, method, params| {
                if method != CLIENT_METHOD_NAMES.session_update {
                    return;
                }
                if let Ok(update) = serde_json::from_value::<SessionNotification>(params) {
                    lock(&for_updates.updates).push(update);
                }
            },
        );
        Self { peer, shared }
    }

    /// The connection underneath, for calls this client has no method for.
    pub fn peer(&self) -> &Peer {
        &self.peer
    }

    /// Sends `initialize` as a client that can read and write files and run terminals.
    pub fn initialize(&self) -> Result<InitializeResponse, rpc::Error> {
        let params = json!({
            "protocolVersion": 1,
            "clientCapabilities": {
                "fs": {"readTextFile": true, "writeTextFile": true},
                "terminal": true,
            },
            "clientInfo": {"name": "apz-acp-fake-client", "version": env!("CARGO_PKG_VERSION")},
        });
        decode(self.call(AGENT_METHOD_NAMES.initialize, params)?)
    }

    /// Opens a session in `cwd` with no MCP servers.
    pub fn new_session(&self, cwd: &str) -> Result<SessionId, rpc::Error> {
        let params = json!({"cwd": cwd, "mcpServers": []});
        let response: NewSessionResponse =
            decode(self.call(AGENT_METHOD_NAMES.session_new, params)?)?;
        Ok(response.session_id)
    }

    /// Sends one text prompt and waits for the turn to end. Every update the agent sent before the end is
    /// already in [`FakeClient::updates`] when this returns.
    pub fn prompt(&self, session: &SessionId, text: &str) -> Result<StopReason, rpc::Error> {
        let params = json!({
            "sessionId": session,
            "prompt": [{"type": "text", "text": text}],
        });
        let response: PromptResponse =
            decode(self.call(AGENT_METHOD_NAMES.session_prompt, params)?)?;
        Ok(response.stop_reason)
    }

    /// Sends `session/cancel` for the session. It is a notification: the turn ends with `cancelled` later.
    pub fn cancel(&self, session: &SessionId) -> Result<(), rpc::Error> {
        self.peer.notify(
            AGENT_METHOD_NAMES.session_cancel,
            json!({"sessionId": session}),
        )
    }

    /// Every `session/update` the agent has sent, in arrival order.
    pub fn updates(&self) -> Vec<SessionNotification> {
        lock(&self.shared.updates).clone()
    }

    /// Every request the agent has made of this client, as (method, params), in arrival order.
    pub fn agent_requests(&self) -> Vec<(String, Value)> {
        lock(&self.shared.requests).clone()
    }

    /// The file the agent left behind, as the client holds it now.
    pub fn file(&self, path: &str) -> Option<String> {
        lock(&self.shared.files).get(path).cloned()
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, rpc::Error> {
        self.peer.call(method, params, CALL_TIMEOUT)
    }
}

/// Answers one request the agent made of this client.
fn answer(
    shared: &Shared,
    config: &ClientConfig,
    method: &str,
    params: Value,
) -> Result<Value, types::Error> {
    lock(&shared.requests).push((method.to_owned(), params.clone()));
    let names = &CLIENT_METHOD_NAMES;
    if method == names.session_request_permission {
        let request: RequestPermissionRequest = from_params(params)?;
        let outcome = match config.permission {
            PermissionAnswer::Kind(kind) => request
                .options
                .iter()
                .find(|option| option.kind == kind)
                .map_or(RequestPermissionOutcome::Cancelled, |option| {
                    RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                        option.option_id.clone(),
                    ))
                }),
            PermissionAnswer::Cancelled => RequestPermissionOutcome::Cancelled,
        };
        return to_value(RequestPermissionResponse::new(outcome));
    }
    if method == names.fs_read_text_file {
        let request: ReadTextFileRequest = from_params(params)?;
        let path = request.path.to_string_lossy().into_owned();
        let files = lock(&shared.files);
        let content = files
            .get(&path)
            .ok_or_else(|| types::Error::resource_not_found(Some(path.clone())))?;
        return Ok(json!({"content": content}));
    }
    if method == names.fs_write_text_file {
        let request: WriteTextFileRequest = from_params(params)?;
        let path = request.path.to_string_lossy().into_owned();
        lock(&shared.files).insert(path, request.content);
        return Ok(json!({}));
    }
    if method == names.terminal_create {
        let _: CreateTerminalRequest = from_params(params)?;
        return Ok(json!({"terminalId": "fake-term-1"}));
    }
    if method == names.terminal_wait_for_exit {
        let _: WaitForTerminalExitRequest = from_params(params)?;
        return Ok(json!({"exitCode": config.terminal_exit_code}));
    }
    if method == names.terminal_output {
        return Ok(json!({
            "output": config.terminal_output,
            "truncated": false,
            "exitStatus": {"exitCode": config.terminal_exit_code},
        }));
    }
    if method == names.terminal_release || method == names.terminal_kill {
        return Ok(json!({}));
    }
    Err(types::Error::method_not_found())
}

fn from_params<T: DeserializeOwned>(params: Value) -> Result<T, types::Error> {
    serde_json::from_value(params)
        .map_err(|err| types::Error::invalid_params().data(json!(err.to_string())))
}

fn to_value<T: Serialize>(value: T) -> Result<Value, types::Error> {
    serde_json::to_value(value).map_err(types::Error::into_internal_error)
}

/// Decodes a reply. A reply that does not fit the schema is reported as an internal error.
fn decode<T: DeserializeOwned>(value: Value) -> Result<T, rpc::Error> {
    serde_json::from_value(value).map_err(|err| {
        rpc::Error::Remote(types::Error::internal_error().data(json!(err.to_string())))
    })
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The command line that starts `binary`, a helper the tests built for the target they run on (the fake
/// agent): the binary alone, or cargo's runner for this target first when one is set
/// (`CARGO_TARGET_<TRIPLE>_RUNNER`, as CI sets `qemu-aarch64 -L …` for the arm64 tests on an x86_64 host).
/// The runner starts only the test binary itself; a child the test starts would be handed to the kernel,
/// which cannot run it on a host with no emulator registered for that architecture.
pub fn helper_argv(binary: &std::path::Path) -> Vec<String> {
    let mut argv: Vec<String> = target_runner(|name| std::env::var(name).ok())
        .map(|runner| runner.split_whitespace().map(String::from).collect())
        .unwrap_or_default();
    argv.push(binary.display().to_string());
    argv
}

/// Cargo's runner for the target these tests were built for, as `var` reads it: Linux GNU targets only,
/// which are the ones the CI runs under an emulator.
fn target_runner(var: impl Fn(&str) -> Option<String>) -> Option<String> {
    if !cfg!(all(target_os = "linux", target_env = "gnu")) {
        return None;
    }
    let name = format!(
        "CARGO_TARGET_{}_UNKNOWN_LINUX_GNU_RUNNER",
        std::env::consts::ARCH.to_uppercase()
    );
    var(&name).filter(|runner| !runner.trim().is_empty())
}

#[cfg(test)]
mod helper_tests {
    use super::*;

    #[test]
    fn a_helper_runs_under_the_targets_runner_only_when_one_is_set() {
        let none = target_runner(|_| None);
        assert_eq!(none, None);
        let blank = target_runner(|_| Some("  ".to_string()));
        assert_eq!(blank, None);
        let asked = std::cell::RefCell::new(String::new());
        let set = target_runner(|name| {
            *asked.borrow_mut() = name.to_string();
            Some("qemu-aarch64 -L /usr/aarch64-linux-gnu".to_string())
        });
        if cfg!(all(target_os = "linux", target_env = "gnu")) {
            assert_eq!(
                set.as_deref(),
                Some("qemu-aarch64 -L /usr/aarch64-linux-gnu")
            );
            let arch = std::env::consts::ARCH.to_uppercase();
            assert_eq!(
                *asked.borrow(),
                format!("CARGO_TARGET_{arch}_UNKNOWN_LINUX_GNU_RUNNER")
            );
        } else {
            assert_eq!(set, None);
        }
        assert_eq!(
            helper_argv(std::path::Path::new("/bin/fake"))
                .last()
                .map(String::as_str),
            Some("/bin/fake")
        );
    }
}
