//! A sandbox's in-VM daemon, compatible with E2B's `envd`: the process and filesystem services the
//! E2B SDKs call, plus file upload and download.
//!
//! A sandbox is a plaza VM started with `plaza.sandbox=1`: the harness runs no agent session; it
//! serves this daemon on vsock port [`PORT`] (the port E2B's envd listens on), and the hub's gateway
//! (vsockd, routing by the SDK's `E2b-Sandbox-Id` header) relays the SDK's HTTP/1.1 requests to it
//! over the VM's vsock device. The guest still has no network device: its outbound traffic goes
//! through the egress proxy and the plaza's laws like an agent's.
//!
//! **Protocol.** Connect RPC with the JSON codec (E2B's SDKs use `useBinaryFormat: false`): a unary
//! call is a `POST /<package>.<Service>/<Method>` with `application/json` bodies, answered with
//! `200` and the response message, or an error status with `{code, message}`; a server-streaming
//! call sends one enveloped message (`application/connect+json`: a flags byte, a big-endian u32
//! length, the JSON) and is answered with enveloped messages and a final end-of-stream envelope
//! (flags `0x02`, `{}` or `{"error": {code, message}}`). Field names are protobuf's JSON names
//! (lowerCamelCase), `bytes` are base64, 64-bit integers are strings, enums are their names. The
//! message shapes are E2B's `spec/envd/{process,filesystem}/*.proto` (Apache-2.0, Copyright E2B);
//! the REST part (`/files`, `/health`, `/init`) is its `spec/envd/envd.yaml`.
//!
//! - `mod.rs` (this file): the vsock listener, the HTTP/1.1 server, Connect framing and routing.
//! - [`process`]: `process.Process/*` — start, list, connect, input, signals, PTY.
//! - [`filesystem`]: `filesystem.Filesystem/*` — stat, list, mkdir, move, remove, watch.
//! - [`files`]: `GET/POST /files` — download and upload.

use std::io;
use std::path::PathBuf;

use serde_json::Value;

pub mod files;
pub mod filesystem;
pub mod process;

/// The vsock port the daemon listens on (E2B's envd port).
pub const PORT: u32 = 49983;
/// The largest request body the server reads (an upload larger than this is refused with 413).
pub const MAX_BODY: usize = 512 << 20;

/// What a handler needs about the sandbox.
#[derive(Debug, Clone)]
pub struct Ctx {
    /// The default working directory and the user's home (E2B's `/home/user`).
    pub home: PathBuf,
    /// The uid and gid commands and file operations run as (the guest's agent user, 1000), unless
    /// a request names `root`.
    pub uid: u32,
    pub gid: u32,
    /// Environment variables every process gets (set by `POST /init` and at start).
    pub env: Vec<(String, String)>,
}

/// One HTTP request, body read in full.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub method: String,
    /// The path without the query.
    pub path: String,
    /// The raw query string (after `?`), empty when none.
    pub query: String,
    /// Header names lower-cased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// The first header named `name` (lower-case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// A Connect error: its code (`invalid_argument`, `not_found`, `already_exists`,
/// `permission_denied`, `failed_precondition`, `internal`, `unimplemented`, …) and message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcError {
    pub code: &'static str,
    pub message: String,
}

impl RpcError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// The error a filesystem operation's `io::Error` maps to.
    pub fn io(err: &io::Error, what: &str) -> Self {
        let code = match err.kind() {
            io::ErrorKind::NotFound => "not_found",
            io::ErrorKind::AlreadyExists => "already_exists",
            io::ErrorKind::PermissionDenied => "permission_denied",
            io::ErrorKind::InvalidInput => "invalid_argument",
            _ => "internal",
        };
        Self::new(code, format!("{what}: {err}"))
    }
}

/// Where a server-streaming handler writes its messages.
pub trait StreamSink {
    /// Sends one response message.
    fn message(&mut self, message: &Value) -> io::Result<()>;
}

/// An HTTP response the server writes.
pub enum Reply {
    /// A complete response.
    Full {
        status: u16,
        content_type: String,
        body: Vec<u8>,
    },
}

impl Reply {
    pub fn json(status: u16, value: &Value) -> Self {
        Reply::Full {
            status,
            content_type: "application/json".to_string(),
            body: serde_json::to_vec(value).unwrap_or_default(),
        }
    }
}
