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

use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, PoisonError, RwLock};
use std::thread;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};

use crate::vsock::VsockListener;

pub mod files;
pub mod filesystem;
pub mod process;

/// The vsock port the daemon listens on (E2B's envd port).
pub const PORT: u32 = 49983;
/// The largest request body the server reads (an upload larger than this is refused with 413).
pub const MAX_BODY: usize = 512 << 20;
/// The largest request head (request line and headers) the server reads.
pub const MAX_HEAD: usize = 16 << 10;

/// The Connect service names the daemon routes (the proto packages' names).
const PROCESS_SERVICE: &str = "process.Process";
const FILESYSTEM_SERVICE: &str = "filesystem.Filesystem";

/// The end-of-stream flag of a Connect envelope.
const FLAG_END: u8 = 0x02;
/// How long the accept loop waits after a failed accept before it tries again.
const ACCEPT_RETRY: Duration = Duration::from_millis(100);

/// The daemon's context, shared by every connection; `POST /init` updates it.
pub type Shared = Arc<RwLock<Ctx>>;

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

    /// An empty `204 No Content`.
    fn empty(status: u16) -> Self {
        Reply::Full {
            status,
            content_type: String::new(),
            body: Vec::new(),
        }
    }
}

/// Serves the daemon on vsock `port` forever, one thread per connection.
///
/// Returns only when the listener cannot be bound; a failed accept is printed and retried.
pub fn serve(port: u32, ctx: Ctx, template: bool) -> io::Result<()> {
    let listener = VsockListener::bind(port)?;
    let shared: Shared = Arc::new(RwLock::new(ctx));
    println!("agent-harness: envd: serving on vsock port {port}");
    // A warm template is snapshotted once its listener is up: the listener survives the restore
    // (only connected sockets are reset), so a sandbox restored from it serves at once.
    if template {
        println!("agent-harness: template ready");
    }
    loop {
        match listener.accept() {
            Ok(stream) => {
                let shared = Arc::clone(&shared);
                thread::spawn(move || {
                    if let Err(err) = handle_connection(stream, &shared) {
                        println!("agent-harness: envd: connection ended: {err}");
                    }
                });
            }
            Err(err) => {
                println!("agent-harness: envd: accept failed: {err}");
                thread::sleep(ACCEPT_RETRY);
            }
        }
    }
}

/// Serves one client connection: its requests in order, until the client closes it, a request
/// asks for `Connection: close`, or a streaming call ends (a stream closes the connection).
///
/// Generic over the stream so the tests can drive it with a socketpair; the daemon passes a
/// vsock stream.
pub fn handle_connection<S: Read + Write>(stream: S, shared: &Shared) -> io::Result<()> {
    let mut reader = BufReader::new(stream);
    loop {
        let (request, keep_alive) = match read_request(&mut reader) {
            Ok(Some(read)) => read,
            Ok(None) => return Ok(()),
            Err(ReadError::Refused(status, err)) => {
                write_reply(reader.get_mut(), &error_reply_with(status, &err), false)?;
                return Ok(());
            }
            Err(ReadError::Io(err)) => return Err(err),
        };
        if let Some((service, method)) = rpc_target(&request.path) {
            if request.method != "POST" {
                let reply = Reply::json(
                    405,
                    &json!({"code": "unimplemented", "message": "RPCs are POST requests"}),
                );
                write_reply(reader.get_mut(), &reply, keep_alive)?;
            } else if is_connect_stream(&request) {
                let ctx = request_context(&request, shared);
                return stream_rpc(reader.get_mut(), &request, &ctx, service, method);
            } else {
                let ctx = request_context(&request, shared);
                let reply = match unary_body(&request.body) {
                    Ok(message) => match rpc_unary(service, method, &message, &ctx) {
                        Ok(value) => Reply::json(200, &value),
                        Err(err) => error_reply(&err),
                    },
                    Err(err) => error_reply(&err),
                };
                write_reply(reader.get_mut(), &reply, keep_alive)?;
            }
        } else {
            let reply = dispatch(&request, shared);
            write_reply(reader.get_mut(), &reply, keep_alive)?;
        }
        if !keep_alive {
            return Ok(());
        }
    }
}

/// The REST routes: `GET /health`, `POST /init`, `GET|POST /files`. The relay in front may strip a
/// prefix, so a route is matched on the path's last segment.
fn dispatch(request: &Request, shared: &Shared) -> Reply {
    match (request.method.as_str(), last_segment(&request.path)) {
        ("GET", "health") => Reply::empty(204),
        ("POST", "init") => init(request, shared),
        ("GET" | "POST", "files") => {
            let ctx = read_lock(shared).clone();
            files::handle(request, &ctx)
        }
        _ => Reply::json(
            404,
            &json!({
                "code": "not_found",
                "message": format!("no route for {} {}", request.method, request.path),
            }),
        ),
    }
}

/// `POST /init`: merges `envVars` into the context every later process gets, sets the clock when
/// `timestamp` (RFC 3339) is given, and mixes `entropy` (64 hex digits; a plaza extension) into the
/// kernel pool. The other fields (`accessToken`, `defaultUser`,
/// `defaultWorkdir`) are accepted and not applied: the sandbox's user and home are fixed.
fn init(request: &Request, shared: &Shared) -> Reply {
    let body = match unary_body(&request.body) {
        Ok(body) => body,
        Err(err) => return error_reply(&err),
    };
    let mut vars = Vec::new();
    if let Some(map) = body.get("envVars").and_then(Value::as_object) {
        for (key, value) in map {
            match value.as_str() {
                Some(value) if !key.is_empty() => vars.push((key.clone(), value.to_string())),
                _ => {
                    return error_reply(&RpcError::new(
                        "invalid_argument",
                        format!("envVars.{key} must be a string"),
                    ));
                }
            }
        }
    }
    if !vars.is_empty() {
        let mut ctx = write_lock(shared);
        for (key, value) in vars {
            ctx.env.retain(|(name, _)| *name != key);
            ctx.env.push((key, value));
        }
    }
    if let Some(timestamp) = body.get("timestamp").and_then(Value::as_str) {
        if let Err(err) = set_clock(timestamp) {
            return error_reply(&err);
        }
    }
    // The plaza's own field: a sandbox starts as a restore of a warm template (and wakes as a
    // restore of its snapshot), so the hub sends fresh entropy with every `/init` and no two
    // sandboxes share the template's random state.
    if let Some(entropy) = body.get("entropy").and_then(Value::as_str) {
        if let Err(err) = crate::restore::mix_hex_entropy(entropy) {
            println!("agent-harness: envd: host entropy not mixed: {err}");
        }
    }
    Reply::empty(204)
}

/// Sets the system clock to `timestamp` with `date -u -s`. The text is checked to be a plain
/// timestamp first, so nothing but digits and the RFC 3339 punctuation reaches the command.
fn set_clock(timestamp: &str) -> Result<(), RpcError> {
    let ms = rfc3339_ms(timestamp)
        .ok_or_else(|| RpcError::new("invalid_argument", "timestamp must be an RFC 3339 time"))?;
    // Only a clock that is off is set (as for an agent's welcome): a sandbox that booted cold is
    // already right, and a restored one is off by however long its template or snapshot waited.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });
    if now.abs_diff(ms) <= 1_000 {
        return Ok(());
    }
    crate::restore::set_realtime_clock(ms)
        .map_err(|err| RpcError::new("internal", format!("cannot set the clock: {err}")))
}

/// Milliseconds since the Unix epoch of an RFC 3339 time (`2026-10-15T03:30:00.123Z`, an offset such
/// as `+02:00` instead of `Z`, fractional seconds optional), `None` when it is not one.
fn rfc3339_ms(text: &str) -> Option<u64> {
    let bytes = text.as_bytes();
    let num = |from: usize, len: usize| -> Option<i64> {
        let part = text.get(from..from + len)?;
        part.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    if bytes.len() < 20
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't' | b' ')
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let (year, month, day) = (num(0, 4)?, num(5, 2)?, num(8, 2)?);
    let (hour, minute, second) = (num(11, 2)?, num(14, 2)?, num(17, 2)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut at = 19;
    let mut millis = 0i64;
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        let digits = bytes[at..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if digits == 0 {
            return None;
        }
        let frac = &text[at..at + digits.min(3)];
        millis = frac.parse::<i64>().ok()? * 10i64.pow(3 - frac.len() as u32);
        at += digits;
    }
    let offset_min = match bytes.get(at)? {
        b'Z' | b'z' if at + 1 == bytes.len() => 0,
        sign @ (b'+' | b'-') if at + 6 == bytes.len() && bytes[at + 3] == b':' => {
            let minutes = num(at + 1, 2)? * 60 + num(at + 4, 2)?;
            if *sign == b'+' { minutes } else { -minutes }
        }
        _ => return None,
    };
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_min * 60;
    u64::try_from(seconds * 1_000 + millis).ok()
}

/// The `service` and `method` of a Connect RPC path (`/process.Process/Start`, possibly behind a
/// prefix), when the service is one the daemon serves.
fn rpc_target(path: &str) -> Option<(&str, &str)> {
    let mut parts = path.rsplit('/');
    let method = parts.next().filter(|method| !method.is_empty())?;
    let service = parts.next()?;
    matches!(service, PROCESS_SERVICE | FILESYSTEM_SERVICE).then_some((service, method))
}

fn last_segment(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or("")
}

/// Whether a Connect call is server-streaming: its request content type says so.
fn is_connect_stream(request: &Request) -> bool {
    request
        .header("content-type")
        .is_some_and(|value| value.starts_with("application/connect+json"))
}

/// A snapshot of the context for one request. A request that names `root` (in a basic
/// `Authorization` header, whose user is `root`, or a `username=root` query) runs as root.
fn request_context(request: &Request, shared: &Shared) -> Ctx {
    let mut ctx = read_lock(shared).clone();
    if asks_for_root(request) {
        ctx.uid = 0;
        ctx.gid = 0;
    }
    ctx
}

fn asks_for_root(request: &Request) -> bool {
    if request.query.split('&').any(|pair| pair == "username=root") {
        return true;
    }
    request
        .header("authorization")
        .and_then(|value| value.strip_prefix("Basic "))
        .and_then(|encoded| {
            base64::engine::general_purpose::STANDARD
                .decode(encoded.trim())
                .ok()
        })
        .is_some_and(|credentials| credentials.starts_with(b"root:"))
}

/// Calls a unary RPC; `Err` is the Connect error.
fn rpc_unary(service: &str, method: &str, message: &Value, ctx: &Ctx) -> Result<Value, RpcError> {
    match service {
        PROCESS_SERVICE => process::unary(method, message, ctx),
        FILESYSTEM_SERVICE => filesystem::unary(method, message, ctx),
        _ => Err(RpcError::new(
            "unimplemented",
            format!("{service}/{method}"),
        )),
    }
}

/// Calls a server-streaming RPC, writing its messages to `sink`.
fn rpc_stream(
    service: &str,
    method: &str,
    message: &Value,
    ctx: &Ctx,
    sink: &mut dyn StreamSink,
) -> Result<(), RpcError> {
    match service {
        PROCESS_SERVICE => process::stream(method, message, ctx, sink),
        FILESYSTEM_SERVICE => filesystem::stream(method, message, ctx, sink),
        _ => Err(RpcError::new(
            "unimplemented",
            format!("{service}/{method}"),
        )),
    }
}

/// A unary request's message: the JSON body, or `{}` when there is none.
fn unary_body(body: &[u8]) -> Result<Value, RpcError> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(json!({}));
    }
    serde_json::from_slice(body)
        .map_err(|err| RpcError::new("invalid_argument", format!("the body is not JSON: {err}")))
}

/// Answers a server-streaming call: `200` with the Connect envelope stream, one chunk per
/// envelope, and a final end-of-stream envelope that carries the call's error, if any.
fn stream_rpc<W: Write>(
    out: &mut W,
    request: &Request,
    ctx: &Ctx,
    service: &str,
    method: &str,
) -> io::Result<()> {
    out.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: application/connect+json\r\n\
          Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
    )?;
    let result = if service == PROCESS_SERVICE && method == "StreamInput" {
        stream_input_rpc(&mut *out, &request.body)?
    } else {
        match decode_one(&request.body) {
            Ok((0, payload)) => match serde_json::from_slice::<Value>(payload) {
                Ok(message) => {
                    let mut sink = EnvelopeSink { out: &mut *out };
                    rpc_stream(service, method, &message, ctx, &mut sink)
                }
                Err(err) => Err(RpcError::new(
                    "invalid_argument",
                    format!("the message is not JSON: {err}"),
                )),
            },
            Ok(_) => Err(RpcError::new(
                "invalid_argument",
                "a request envelope must be an uncompressed message",
            )),
            Err(reason) => Err(RpcError::new("invalid_argument", reason)),
        }
    };
    write_chunk(out, &end_envelope(result.as_ref().err()))?;
    out.write_all(b"0\r\n\r\n")?;
    out.flush()
}

/// Answers `StreamInput`, the client-streaming call: the request's envelopes are its messages. On
/// success the answer's one `{}` message is written here; the caller writes the end envelope.
fn stream_input_rpc<W: Write>(out: &mut W, body: &[u8]) -> io::Result<Result<(), RpcError>> {
    let result = stream_input_messages(body).and_then(|messages| process::stream_input(&messages));
    if result.is_ok() {
        write_chunk(out, &encode_envelope(0, b"{}"))?;
    }
    Ok(result)
}

/// The messages of a `StreamInput` body: each request envelope's JSON, in order. An end-of-stream
/// envelope (flags `0x02`) carries nothing and is skipped.
fn stream_input_messages(body: &[u8]) -> Result<Vec<Value>, RpcError> {
    let envelopes = decode_all(body).map_err(|reason| RpcError::new("invalid_argument", reason))?;
    let mut messages = Vec::new();
    for (flags, payload) in envelopes {
        match flags {
            FLAG_END => {}
            0 => messages.push(serde_json::from_slice::<Value>(payload).map_err(|err| {
                RpcError::new(
                    "invalid_argument",
                    format!("the message is not JSON: {err}"),
                )
            })?),
            _ => {
                return Err(RpcError::new(
                    "invalid_argument",
                    "a request envelope must be an uncompressed message",
                ));
            }
        }
    }
    Ok(messages)
}

/// Writes each message as one envelope in its own HTTP chunk.
struct EnvelopeSink<'a, W: Write> {
    out: &'a mut W,
}

impl<W: Write> StreamSink for EnvelopeSink<'_, W> {
    fn message(&mut self, message: &Value) -> io::Result<()> {
        let body = serde_json::to_vec(message)?;
        write_chunk(&mut *self.out, &encode_envelope(0, &body))
    }
}

fn write_chunk<W: Write>(out: &mut W, bytes: &[u8]) -> io::Result<()> {
    write!(out, "{:x}\r\n", bytes.len())?;
    out.write_all(bytes)?;
    out.write_all(b"\r\n")
}

/// One Connect envelope: a flags byte, the body's length as a big-endian u32, then the body.
pub fn encode_envelope(flags: u8, body: &[u8]) -> Vec<u8> {
    let length = u32::try_from(body.len()).expect("an envelope body fits in a u32");
    let mut out = Vec::with_capacity(5 + body.len());
    out.push(flags);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// Splits the envelope at the front of `bytes`: its flags, its body, and the bytes after it.
fn split_envelope(bytes: &[u8]) -> Result<(u8, &[u8], &[u8]), String> {
    if bytes.len() < 5 {
        return Err("the envelope is shorter than its 5-byte header".to_string());
    }
    let length = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]) as usize;
    let rest = &bytes[5..];
    if rest.len() < length {
        return Err(format!(
            "the envelope says {length} bytes but carries {}",
            rest.len()
        ));
    }
    let (payload, after) = rest.split_at(length);
    Ok((bytes[0], payload, after))
}

/// Decodes bytes that are exactly one envelope into its flags and body.
pub fn decode_one(bytes: &[u8]) -> Result<(u8, &[u8]), String> {
    let (flags, payload, after) = split_envelope(bytes)?;
    if !after.is_empty() {
        return Err(format!(
            "the envelope says {} bytes but carries {}",
            payload.len(),
            payload.len() + after.len()
        ));
    }
    Ok((flags, payload))
}

/// Decodes bytes that are a run of envelopes: each one's flags and body, in order.
pub fn decode_all(mut bytes: &[u8]) -> Result<Vec<(u8, &[u8])>, String> {
    let mut envelopes = Vec::new();
    while !bytes.is_empty() {
        let (flags, payload, after) = split_envelope(bytes)?;
        envelopes.push((flags, payload));
        bytes = after;
    }
    Ok(envelopes)
}

/// The end-of-stream envelope: `{}`, or `{"error": {code, message}}` when the call failed.
pub fn end_envelope(error: Option<&RpcError>) -> Vec<u8> {
    let body = match error {
        None => json!({}),
        Some(err) => json!({"error": {"code": err.code, "message": err.message}}),
    };
    encode_envelope(FLAG_END, &serde_json::to_vec(&body).unwrap_or_default())
}

/// The HTTP status a Connect error code answers a unary call with.
pub fn status_of(code: &str) -> u16 {
    match code {
        "invalid_argument" => 400,
        "not_found" => 404,
        "already_exists" => 409,
        "permission_denied" => 403,
        "unimplemented" => 501,
        _ => 500,
    }
}

fn error_reply(err: &RpcError) -> Reply {
    Reply::json(
        status_of(err.code),
        &json!({"code": err.code, "message": err.message}),
    )
}

/// A reply for a request the server refuses, with the status it is refused with.
fn error_reply_with(status: u16, err: &RpcError) -> Reply {
    Reply::json(status, &json!({"code": err.code, "message": err.message}))
}

/// Why a request could not be read: a refusal (the status and the error to answer with), or the
/// connection failing, in which case nothing is answered.
enum ReadError {
    Refused(u16, RpcError),
    Io(io::Error),
}

impl From<io::Error> for ReadError {
    fn from(err: io::Error) -> Self {
        ReadError::Io(err)
    }
}

fn refused(status: u16, code: &'static str, message: &str) -> ReadError {
    ReadError::Refused(status, RpcError::new(code, message))
}

/// Reads one request, its body in full. `Ok(None)` is a client that closed the connection between
/// requests. The flag is whether the connection stays open after the answer (HTTP/1.1 without
/// `Connection: close`).
fn read_request<R: BufRead>(reader: &mut R) -> Result<Option<(Request, bool)>, ReadError> {
    let mut budget = MAX_HEAD;
    let line = read_line(reader, &mut budget)?;
    if line.is_empty() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&line).into_owned();
    let mut parts = text.split_ascii_whitespace();
    let (method, target, version) = match (parts.next(), parts.next(), parts.next()) {
        (Some(method), Some(target), Some(version))
            if version == "HTTP/1.1" || version == "HTTP/1.0" =>
        {
            (method.to_string(), target.to_string(), version.to_string())
        }
        _ => return Err(refused(400, "invalid_argument", "malformed request line")),
    };
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let mut request = Request {
        method,
        path: path.to_string(),
        query: query.to_string(),
        headers: Vec::new(),
        body: Vec::new(),
    };
    loop {
        let line = read_line(reader, &mut budget)?;
        if line.is_empty() {
            return Err(ReadError::Io(io::ErrorKind::UnexpectedEof.into()));
        }
        let text = String::from_utf8_lossy(&line).into_owned();
        let header = text.trim_end();
        if header.is_empty() {
            break;
        }
        let Some((name, value)) = header.split_once(':') else {
            return Err(refused(400, "invalid_argument", "malformed header"));
        };
        request
            .headers
            .push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
    let keep_alive = version == "HTTP/1.1"
        && !request
            .header("connection")
            .is_some_and(|value| value.eq_ignore_ascii_case("close"));
    request.body = read_body(reader, &request)?;
    Ok(Some((request, keep_alive)))
}

/// One line, its terminator included, read against the head's remaining `budget`. An empty line
/// is end of input.
fn read_line<R: BufRead>(reader: &mut R, budget: &mut usize) -> Result<Vec<u8>, ReadError> {
    let mut line = Vec::new();
    let read = reader
        .by_ref()
        .take(*budget as u64 + 1)
        .read_until(b'\n', &mut line)?;
    if read > *budget {
        return Err(refused(
            431,
            "invalid_argument",
            "the request head is over 16 KiB",
        ));
    }
    *budget -= read;
    Ok(line)
}

fn read_body<R: BufRead>(reader: &mut R, request: &Request) -> Result<Vec<u8>, ReadError> {
    let chunked = request
        .header("transfer-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"));
    if chunked {
        return read_chunked(reader);
    }
    let length = match request.header("content-length") {
        None => 0,
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| refused(400, "invalid_argument", "Content-Length is not a number"))?,
    };
    if length > MAX_BODY {
        return Err(refused(
            413,
            "resource_exhausted",
            "the request body is larger than the daemon reads",
        ));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(body)
}

fn read_chunked<R: BufRead>(reader: &mut R) -> Result<Vec<u8>, ReadError> {
    let mut body = Vec::new();
    loop {
        let mut budget = MAX_HEAD;
        let line = read_line(reader, &mut budget)?;
        let text = String::from_utf8_lossy(&line).into_owned();
        let digits = text.trim().split(';').next().unwrap_or("");
        let size = usize::from_str_radix(digits, 16)
            .map_err(|_| refused(400, "invalid_argument", "a chunk size is not hexadecimal"))?;
        if size == 0 {
            // The trailers, up to the blank line that ends the body.
            loop {
                let mut budget = MAX_HEAD;
                let line = read_line(reader, &mut budget)?;
                if line.iter().all(u8::is_ascii_whitespace) {
                    return Ok(body);
                }
            }
        }
        if body.len() + size > MAX_BODY {
            return Err(refused(
                413,
                "resource_exhausted",
                "the request body is larger than the daemon reads",
            ));
        }
        let start = body.len();
        body.resize(start + size, 0);
        reader.read_exact(&mut body[start..])?;
        let mut budget = MAX_HEAD;
        let end_of_chunk = read_line(reader, &mut budget)?;
        if !end_of_chunk.iter().all(u8::is_ascii_whitespace) {
            return Err(refused(
                400,
                "invalid_argument",
                "a chunk does not end where it should",
            ));
        }
    }
}

/// Writes a complete response. A `204` has no body and no length.
fn write_reply<W: Write>(out: &mut W, reply: &Reply, keep_alive: bool) -> io::Result<()> {
    let Reply::Full {
        status,
        content_type,
        body,
    } = reply;
    let connection = if keep_alive { "keep-alive" } else { "close" };
    let mut head = format!(
        "HTTP/1.1 {status} {}\r\nConnection: {connection}\r\n",
        reason(*status)
    );
    if *status == 204 || *status == 304 {
        head.push_str("\r\n");
        out.write_all(head.as_bytes())?;
    } else {
        head.push_str(&format!(
            "Content-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",
            body.len()
        ));
        out.write_all(head.as_bytes())?;
        out.write_all(body)?;
    }
    out.flush()
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        501 => "Not Implemented",
        _ => "Internal Server Error",
    }
}

fn read_lock(shared: &Shared) -> std::sync::RwLockReadGuard<'_, Ctx> {
    shared.read().unwrap_or_else(PoisonError::into_inner)
}

fn write_lock(shared: &Shared) -> std::sync::RwLockWriteGuard<'_, Ctx> {
    shared.write().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::io::{self, Cursor, Read, Write};
    use std::os::unix::fs::MetadataExt;
    use std::sync::{Arc, RwLock};
    use std::thread;
    use std::time::{Duration, Instant};

    use serde_json::{Value, json};

    use super::{Ctx, Shared, encode_envelope, handle_connection};

    /// A context for tests: the test user's own uid and gid (a process may drop to those without
    /// privilege), and the temp directory as home.
    pub fn test_ctx() -> Ctx {
        let owner = std::fs::metadata("/proc/self").expect("/proc/self");
        Ctx {
            home: std::env::temp_dir(),
            uid: owner.uid(),
            gid: owner.gid(),
            env: Vec::new(),
        }
    }

    pub fn shared_of(ctx: &Ctx) -> Shared {
        Arc::new(RwLock::new(ctx.clone()))
    }

    /// A connection that reads `input` and keeps what is written to it.
    pub struct Duplex {
        input: Cursor<Vec<u8>>,
        pub output: Vec<u8>,
    }

    impl Read for Duplex {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for Duplex {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Serves one connection carrying `request` bytes, and returns the response bytes.
    pub fn exchange(shared: &Shared, request: &[u8]) -> Vec<u8> {
        let mut duplex = Duplex {
            input: Cursor::new(request.to_vec()),
            output: Vec::new(),
        };
        handle_connection(&mut duplex, shared).expect("the connection is served");
        duplex.output
    }

    /// One HTTP/1.1 request with a body, asking the connection to close after its answer.
    pub fn raw(method: &str, path: &str, content_type: &str, body: &[u8]) -> Vec<u8> {
        let mut out = format!(
            "{method} {path} HTTP/1.1\r\nHost: envd\r\nContent-Type: {content_type}\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(body);
        out
    }

    /// A unary call on a fresh connection; the whole response.
    pub fn call(ctx: &Ctx, path: &str, body: Value) -> Vec<u8> {
        let request = raw(
            "POST",
            path,
            "application/json",
            &serde_json::to_vec(&body).expect("JSON"),
        );
        exchange(&shared_of(ctx), &request)
    }

    /// A server-streaming call on a fresh connection: its messages, and its end-of-stream envelope.
    pub fn call_stream(ctx: &Ctx, path: &str, body: Value) -> (Vec<Value>, Value) {
        stream_on(&shared_of(ctx), path, body)
    }

    pub fn stream_on(shared: &Shared, path: &str, body: Value) -> (Vec<Value>, Value) {
        let envelope = encode_envelope(0, &serde_json::to_vec(&body).expect("JSON"));
        let response = exchange(
            shared,
            &raw("POST", path, "application/connect+json", &envelope),
        );
        let (head, body) = split_response(&response);
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        let mut messages = Vec::new();
        let mut end = None;
        let mut rest: &[u8] = &body;
        while !rest.is_empty() {
            let flags = rest[0];
            let length = u32::from_be_bytes([rest[1], rest[2], rest[3], rest[4]]) as usize;
            let value: Value =
                serde_json::from_slice(&rest[5..5 + length]).expect("a JSON envelope");
            if flags == 0x02 {
                end = Some(value);
            } else {
                messages.push(value);
            }
            rest = &rest[5 + length..];
        }
        (messages, end.expect("an end-of-stream envelope"))
    }

    /// The head and the body of a response, the body de-chunked when it is chunked.
    pub fn split_response(response: &[u8]) -> (String, Vec<u8>) {
        let split = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .expect("a response head");
        let head = String::from_utf8_lossy(&response[..split]).into_owned();
        let body = &response[split + 4..];
        if head
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked")
        {
            (head, dechunk(body))
        } else {
            (head, body.to_vec())
        }
    }

    fn dechunk(mut rest: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let eol = rest
                .windows(2)
                .position(|window| window == b"\r\n")
                .expect("a chunk size line");
            let size = usize::from_str_radix(std::str::from_utf8(&rest[..eol]).expect("ASCII"), 16)
                .expect("a hex chunk size");
            rest = &rest[eol + 2..];
            if size == 0 {
                return out;
            }
            out.extend_from_slice(&rest[..size]);
            rest = &rest[size + 2..];
        }
    }

    /// The JSON body of a unary response.
    pub fn json_body(response: &[u8]) -> Value {
        let (_, body) = split_response(response);
        serde_json::from_slice(&body).expect("a JSON body")
    }

    /// Waits until a process started with `tag` is listed.
    pub fn wait_for_tag(ctx: &Ctx, tag: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let listed = json_body(&call(ctx, "/process.Process/List", json!({})));
            let found = listed["processes"]
                .as_array()
                .is_some_and(|list| list.iter().any(|p| p["tag"] == json!(tag)));
            if found {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "no process tagged {tag} was listed"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(test)]
mod rfc3339_tests {
    use super::rfc3339_ms;

    #[test]
    fn rfc3339_times_parse_to_epoch_milliseconds() {
        assert_eq!(rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            rfc3339_ms("2026-10-15T03:30:00.123Z"),
            Some(1_792_035_000_123)
        );
        assert_eq!(
            rfc3339_ms("2026-10-15T05:30:00.5+02:00"),
            Some(1_792_035_000_500)
        );
        assert_eq!(rfc3339_ms("2000-02-29T12:00:00Z"), Some(951_825_600_000));
        for bad in [
            "",
            "2026-10-15",
            "2026-13-01T00:00:00Z",
            "2026-10-15T03:30:00",
            "x026-10-15T03:30:00Z",
        ] {
            assert_eq!(rfc3339_ms(bad), None, "{bad}");
        }
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use serde_json::json;

    use super::test_support::{
        call, exchange, json_body, raw, shared_of, split_response, stream_on, test_ctx,
    };
    use super::*;

    #[test]
    fn envelopes_round_trip_and_a_wrong_length_is_refused() {
        let bytes = encode_envelope(0, b"{\"a\":1}");
        assert_eq!(bytes[0], 0);
        assert_eq!(&bytes[1..5], &7u32.to_be_bytes());
        assert_eq!(decode_one(&bytes), Ok((0, &b"{\"a\":1}"[..])));
        assert!(decode_one(&bytes[..6]).is_err(), "a truncated envelope");
        assert!(decode_one(&[0, 0]).is_err(), "a short header");

        let end = end_envelope(Some(&RpcError::new("not_found", "gone")));
        let (flags, payload) = decode_one(&end).expect("one envelope");
        assert_eq!(flags, FLAG_END);
        assert_eq!(
            serde_json::from_slice::<Value>(payload).expect("JSON"),
            json!({"error": {"code": "not_found", "message": "gone"}})
        );
        assert_eq!(decode_one(&end_envelope(None)).expect("one").1, b"{}");
    }

    #[test]
    fn a_run_of_envelopes_decodes_to_each_of_them() {
        let mut body = encode_envelope(0, b"{\"a\":1}");
        body.extend(encode_envelope(0, b"{}"));
        body.extend(end_envelope(None));
        let envelopes = decode_all(&body).expect("three envelopes");
        assert_eq!(
            envelopes,
            vec![
                (0, &b"{\"a\":1}"[..]),
                (0, &b"{}"[..]),
                (FLAG_END, &b"{}"[..])
            ]
        );
        assert_eq!(decode_all(&[]), Ok(Vec::new()));
        assert!(decode_all(&body[..body.len() - 1]).is_err(), "a truncated run");
    }

    #[test]
    fn stream_input_refuses_a_stream_that_does_not_start_with_start() {
        let shared = shared_of(&test_ctx());
        let mut body = encode_envelope(0, br#"{"data":{"input":{"stdin":""}}}"#);
        body.extend(end_envelope(None));
        let response = exchange(
            &shared,
            &raw(
                "POST",
                "/process.Process/StreamInput",
                "application/connect+json",
                &body,
            ),
        );
        let (head, payload) = split_response(&response);
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        let (flags, end) = decode_one(&payload).expect("the end envelope alone");
        assert_eq!(flags, FLAG_END);
        let end: Value = serde_json::from_slice(end).expect("JSON");
        assert_eq!(end.pointer("/error/code"), Some(&json!("invalid_argument")));
    }

    #[test]
    fn connect_codes_map_to_their_statuses() {
        assert_eq!(status_of("invalid_argument"), 400);
        assert_eq!(status_of("not_found"), 404);
        assert_eq!(status_of("already_exists"), 409);
        assert_eq!(status_of("permission_denied"), 403);
        assert_eq!(status_of("unimplemented"), 501);
        assert_eq!(status_of("internal"), 500);
        assert_eq!(status_of("something_new"), 500);
    }

    #[test]
    fn routes_answer_with_their_status_and_the_connect_error_body() {
        let ctx = test_ctx();
        let shared = shared_of(&ctx);
        let health = exchange(&shared, &raw("GET", "/health", "text/plain", b""));
        assert!(health.starts_with(b"HTTP/1.1 204"));
        let unknown = exchange(&shared, &raw("GET", "/nope", "text/plain", b""));
        assert!(unknown.starts_with(b"HTTP/1.1 404"));
        let get_rpc = exchange(
            &shared,
            b"GET /process.Process/List HTTP/1.1\r\nConnection: close\r\n\r\n",
        );
        assert!(get_rpc.starts_with(b"HTTP/1.1 405"));
        let missing = call(&ctx, "/process.Process/Nope", json!({}));
        assert!(missing.starts_with(b"HTTP/1.1 501"));
        let no_selector = call(&ctx, "/process.Process/SendInput", json!({}));
        assert!(no_selector.starts_with(b"HTTP/1.1 400"));
        assert_eq!(json_body(&no_selector)["code"], json!("invalid_argument"));
        let broken = exchange(
            &shared,
            &raw(
                "POST",
                "/process.Process/List",
                "application/json",
                b"{nope",
            ),
        );
        assert!(broken.starts_with(b"HTTP/1.1 400"));
    }

    #[test]
    fn an_oversized_head_is_refused_and_the_connection_ends() {
        let shared = shared_of(&test_ctx());
        let request = format!(
            "GET /health HTTP/1.1\r\nX-Pad: {}\r\n\r\n",
            "a".repeat(20 << 10)
        );
        let response = exchange(&shared, request.as_bytes());
        assert!(response.starts_with(b"HTTP/1.1 431"));
    }

    #[test]
    fn a_keep_alive_connection_serves_its_requests_in_order() {
        let shared = shared_of(&test_ctx());
        let request = b"GET /health HTTP/1.1\r\nHost: envd\r\n\r\n\
            POST /process.Process/Nope HTTP/1.1\r\nContent-Type: application/json\r\n\
            Content-Length: 2\r\n\r\n{}\
            POST /process.Process/Nope HTTP/1.1\r\nContent-Type: application/json\r\n\
            Transfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n";
        let response = String::from_utf8_lossy(&exchange(&shared, request)).into_owned();
        // Each response starts right after the previous body, so the status lines are found by
        // their marker rather than by line.
        let statuses: Vec<&str> = response
            .split("HTTP/1.1 ")
            .skip(1)
            .map(|rest| rest.lines().next().unwrap_or(""))
            .collect();
        assert_eq!(
            statuses,
            [
                "204 No Content",
                "501 Not Implemented",
                "501 Not Implemented"
            ]
        );
    }

    #[test]
    fn init_sets_the_environment_that_later_processes_get() {
        let shared = shared_of(&test_ctx());
        let init = exchange(
            &shared,
            &raw(
                "POST",
                "/init",
                "application/json",
                br#"{"envVars": {"GREETING": "hi there"}}"#,
            ),
        );
        assert!(init.starts_with(b"HTTP/1.1 204"));
        let (messages, end) = stream_on(
            &shared,
            "/process.Process/Start",
            json!({"process": {"cmd": "sh", "args": ["-c", "echo $GREETING"]}}),
        );
        assert!(end.get("error").is_none(), "{end}");
        let stdout: Vec<u8> = messages
            .iter()
            .filter_map(|m| m.pointer("/event/data/stdout"))
            .flat_map(|data| {
                base64::engine::general_purpose::STANDARD
                    .decode(data.as_str().unwrap_or(""))
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(stdout, b"hi there\n");

        let bad = exchange(
            &shared,
            &raw(
                "POST",
                "/init",
                "application/json",
                br#"{"envVars": {"A": 1}}"#,
            ),
        );
        assert!(bad.starts_with(b"HTTP/1.1 400"));
        let (head, _) = split_response(&bad);
        assert!(head.starts_with("HTTP/1.1 400"));
    }
}
