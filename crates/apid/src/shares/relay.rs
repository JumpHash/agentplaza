//! The relay client: while at least one share is live, a task keeps a signed WebSocket open to agentplaza.io so
//! `https://agentplaza.io/artifact/{id}` reaches this hub without inbound traffic.
//!
//! The hub holds an Ed25519 key in `<state_dir>/relay_key`. Its hub id is the first 32 hex characters of the SHA-256
//! of the public key, and every message is signed over `apz-relay-<kind>` and its parts, joined with newlines. The
//! wire protocol is in `cloudflare/relay/README.md` ("Protocol"). A relayed request runs through the hub's own
//! router, so the answer is the one the network link gives.

use std::io::{ErrorKind, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use apz_http::{Request, Response};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::approvals;
use crate::state::App;

/// The relay the hub talks to unless `APZ_RELAY_URL` names another.
const DEFAULT_BASE: &str = "https://agentplaza.io";

/// Body bytes per `res` frame. Base64 and JSON add a third, so a frame stays under the 1 MiB WebSocket limit.
const CHUNK_BYTES: usize = 512 * 1024;

/// How long the socket may read nothing before it is closed. The relay pings every 30 seconds.
const SILENT_AFTER: Duration = Duration::from_secs(90);

/// How long one socket read may block, so queued frames go out and the live shares are checked about once a second.
const TICK: Duration = Duration::from_secs(1);

/// How often the idle loop looks for a live share.
const IDLE_POLL: Duration = Duration::from_secs(2);

/// The longest wait between reconnects, in seconds.
const BACKOFF_MAX_SECS: f64 = 60.0;

/// Response headers the hub's answer does not pass on: the relay sets the framing and the length itself.
const DROPPED_HEADERS: [&str; 5] = [
    "content-length",
    "connection",
    "keep-alive",
    "transfer-encoding",
    "upgrade",
];

/// The socket the relay client holds to agentplaza.io.
type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

/// The hub's side of the relay: its key, its hub id, whether it has registered, and the frames of the open socket.
pub struct Relay {
    key: SigningKey,
    hub: String,
    base: String,
    registered: AtomicBool,
    /// The sender of the open socket's frames, or `None` while no socket is open.
    outbox: Mutex<Option<Sender<String>>>,
}

/// The relay once `start` has made it. `claim` and `release` send through it.
static RELAY: OnceLock<Relay> = OnceLock::new();

/// The secret a relayed request carries in `x-apz-relay`: 32 random hex characters, made once per process. Only the
/// relay client knows it, so the hub can tell a relayed request from one a network client forged.
static MARKER: OnceLock<String> = OnceLock::new();

/// The relay marker of this process (see `MARKER`). The public share routes compare it with the `x-apz-relay`
/// header of each request.
pub fn relay_marker() -> &'static str {
    MARKER.get_or_init(|| {
        let mut bytes = [0u8; 16];
        apz_os::random::fill(&mut bytes).expect("the relay marker needs OS randomness");
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    })
}

/// The file that holds the hub's relay key: `<state_dir>/relay_key`.
pub fn key_path(state_dir: &Path) -> PathBuf {
    state_dir.join("relay_key")
}

/// The hub id for a public key: the first 32 lower-case hex characters of the SHA-256 of its 32 raw bytes.
pub fn hub_id(public: &[u8; 32]) -> String {
    Sha256::digest(public)
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The base64url signature (no padding) of the bytes `apz-relay-<kind>` and `parts`, joined with newlines.
pub fn signed(key: &SigningKey, kind: &str, parts: &[&str]) -> String {
    URL_SAFE_NO_PAD.encode(key.sign(&signing_bytes(kind, parts)).to_bytes())
}

/// The bytes a signature covers: `apz-relay-<kind>` and the parts, one per line.
fn signing_bytes(kind: &str, parts: &[&str]) -> Vec<u8> {
    let mut lines = vec![format!("apz-relay-{kind}")];
    lines.extend(parts.iter().map(|part| part.to_string()));
    lines.join("\n").into_bytes()
}

/// Starts the background thread that keeps the relay socket open while a share is live. The key is read or created
/// in the state folder. Nothing happens when `APZ_RELAY=off`, and only one client runs per process.
pub fn start(app: Arc<App>) {
    if std::env::var("APZ_RELAY").is_ok_and(|value| value == "off") {
        return;
    }
    let key = match load_key(&key_path(&app.config.state_dir)) {
        Ok(key) => key,
        Err(e) => {
            eprintln!("relay: cannot read or create the hub key: {e}");
            return;
        }
    };
    let base = std::env::var("APZ_RELAY_URL")
        .ok()
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| DEFAULT_BASE.to_string());
    let hub = hub_id(&key.verifying_key().to_bytes());
    let relay = Relay {
        key,
        hub,
        base: base.trim_end_matches('/').to_string(),
        registered: AtomicBool::new(false),
        outbox: Mutex::new(None),
    };
    if RELAY.set(relay).is_err() {
        return;
    }
    if let Err(e) = thread::Builder::new()
        .name("apz-relay".into())
        .spawn(move || run(app))
    {
        eprintln!("relay: cannot start the client thread: {e}");
    }
}

/// Tells the relay that share `id` is live. Does nothing when no socket is open: the next connect claims every live
/// share.
pub fn claim(id: &str) {
    send_share("claim", id);
}

/// Tells the relay that share `id` is revoked or expired. Does nothing when no socket is open.
pub fn release(id: &str) {
    send_share("release", id);
}

/// Sends a signed `claim` or `release` for share `id` through the open socket, if there is one.
fn send_share(kind: &str, id: &str) {
    let Some(relay) = RELAY.get() else {
        return;
    };
    if let Some(tx) = lock(&relay.outbox).as_ref() {
        let _ = tx.send(share_frame(&relay.key, &relay.hub, kind, id, now_secs()));
    }
}

/// The JSON of a signed share message: `{"t": kind, "id", "ts", "sig"}`.
fn share_frame(key: &SigningKey, hub: &str, kind: &str, id: &str, ts: u64) -> String {
    let ts_text = ts.to_string();
    let sig = signed(key, kind, &[hub, id, ts_text.as_str()]);
    json!({"t": kind, "id": id, "ts": ts, "sig": sig}).to_string()
}

/// The client's loop. While a share is live it registers the key once, connects, and serves the socket until it
/// ends. A failed register or connect waits with backoff. With no live share it idles.
fn run(app: Arc<App>) {
    let Some(relay) = RELAY.get() else {
        return;
    };
    let mut attempt = 0u32;
    loop {
        if app.shares.live().is_empty() {
            thread::sleep(IDLE_POLL);
            continue;
        }
        if !relay.registered.load(Ordering::SeqCst)
            && let Err(e) = register(relay)
        {
            eprintln!("relay: register: {e}");
            thread::sleep(backoff(attempt));
            attempt = attempt.saturating_add(1);
            continue;
        }
        match connect(relay) {
            Ok(socket) => {
                attempt = 0;
                if let Err(e) = session(&app, relay, socket) {
                    eprintln!("relay: connection ended: {e}");
                }
            }
            Err(e) => eprintln!("relay: connect: {e}"),
        }
        thread::sleep(backoff(attempt));
        attempt = attempt.saturating_add(1);
    }
}

/// Registers the hub's public key with the relay. A 409 means another key already holds this hub id.
fn register(relay: &Relay) -> Result<(), String> {
    let key = URL_SAFE_NO_PAD.encode(relay.key.verifying_key().to_bytes());
    let ts = now_secs();
    let ts_text = ts.to_string();
    let hub = relay.hub.as_str();
    let sig = signed(
        &relay.key,
        "register",
        &[hub, key.as_str(), ts_text.as_str()],
    );
    let body = json!({"hub": hub, "key": key, "ts": ts, "sig": sig});
    let url = format!("{}/relay/register", relay.base);
    match ureq::post(url.as_str()).send_json(&body) {
        Ok(_) => {
            relay.registered.store(true, Ordering::SeqCst);
            Ok(())
        }
        Err(ureq::Error::StatusCode(409)) => Err("another key holds this hub id (409)".to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// Opens the relay socket for this hub. The connect signature covers `connect`, the hub and the current time.
fn connect(relay: &Relay) -> Result<Socket, String> {
    let ts_text = now_secs().to_string();
    let hub = relay.hub.as_str();
    let sig = signed(&relay.key, "connect", &[hub, ts_text.as_str()]);
    let url = format!(
        "{}/relay/connect?hub={hub}&ts={ts_text}&sig={sig}",
        ws_base(&relay.base)
    );
    let (tls, host, port) =
        apz_http::client::split_base(&origin(&relay.base)).map_err(|e| e.to_string())?;
    let tcp = TcpStream::connect((host.as_str(), port)).map_err(|e| e.to_string())?;
    // TLS with the plaza's own client configuration (the Mozilla roots, ring), not tungstenite's.
    let connector =
        tls.then(|| tungstenite::Connector::Rustls(apz_http::client::public_client_config()));
    let (socket, _) = tungstenite::client_tls_with_config(url.as_str(), tcp, None, connector)
        .map_err(|e| e.to_string())?;
    set_tick(&socket);
    Ok(socket)
}

/// The scheme and authority of an HTTP base, without a path or query: `https://relay.example/x` is
/// `https://relay.example`.
fn origin(base: &str) -> String {
    let (scheme, rest) = base.split_once("://").unwrap_or(("", base));
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    format!("{scheme}://{authority}")
}

/// The WebSocket base of an HTTP base: `https` becomes `wss` and `http` becomes `ws`.
fn ws_base(base: &str) -> String {
    if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    }
}

/// Gives the socket's TCP stream a read timeout, so a read returns every `TICK` when nothing has arrived.
fn set_tick(socket: &Socket) {
    let tcp = match socket.get_ref() {
        MaybeTlsStream::Plain(tcp) => Some(tcp),
        MaybeTlsStream::Rustls(tls) => Some(tls.get_ref()),
        _ => None,
    };
    if let Some(tcp) = tcp {
        let _ = tcp.set_read_timeout(Some(TICK));
    }
}

/// Serves one socket until it fails, goes silent, or no share is live any more. Every live share is claimed first,
/// and the frames queued by `claim`, `release` and the request threads go out between reads.
fn session(app: &Arc<App>, relay: &Relay, mut socket: Socket) -> Result<(), String> {
    let (tx, rx) = mpsc::channel::<String>();
    *lock(&relay.outbox) = Some(tx.clone());
    for share in app.shares.live() {
        let _ = tx.send(share_frame(
            &relay.key,
            &relay.hub,
            "claim",
            &share.id,
            now_secs(),
        ));
    }
    let mut last_in = Instant::now();
    let outcome = loop {
        if let Err(e) = flush(&mut socket, &rx) {
            break Err(e);
        }
        if app.shares.live().is_empty() {
            break Ok(());
        }
        if last_in.elapsed() > SILENT_AFTER {
            break Err("the relay went silent".to_string());
        }
        match socket.read() {
            Ok(Message::Text(text)) => {
                last_in = Instant::now();
                handle(app, &tx, text.as_str());
            }
            Ok(Message::Close(_)) => break Err("the relay closed the socket".to_string()),
            Ok(_) => last_in = Instant::now(),
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => break Err(e.to_string()),
        }
    };
    *lock(&relay.outbox) = None;
    let _ = socket.close(None);
    outcome
}

/// Sends every frame queued on `rx` through the socket.
fn flush(socket: &mut Socket, rx: &Receiver<String>) -> Result<(), String> {
    loop {
        match rx.try_recv() {
            Ok(text) => socket
                .send(Message::text(text))
                .map_err(|e| e.to_string())?,
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => return Ok(()),
        }
    }
}

/// Acts on one text frame from the relay: a ping is answered, a request is served on its own thread, and a report
/// files an approval for the owner. Other frames are acknowledgements or errors, which are named on stderr.
fn handle(app: &Arc<App>, outbox: &Sender<String>, text: &str) {
    let msg: Value = match serde_json::from_str(text) {
        Ok(msg) => msg,
        Err(_) => {
            eprintln!("relay: a frame is not JSON");
            return;
        }
    };
    let kind = msg
        .get("t")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    match kind.as_str() {
        "ping" => {
            let _ = outbox.send(json!({"t": "pong"}).to_string());
        }
        "req" => {
            let app = Arc::clone(app);
            let outbox = outbox.clone();
            thread::spawn(move || serve(&app, &outbox, &msg));
        }
        "reported" => report(app, &msg),
        "error" => eprintln!("relay: {msg}"),
        _ => {}
    }
}

/// Runs one relayed request through the hub and sends the answer back in `res` frames. A request the hub cannot
/// read gets a 400.
fn serve(app: &App, outbox: &Sender<String>, msg: &Value) {
    let rid = msg.get("rid").cloned().unwrap_or(Value::Null);
    let response = match request_of(msg) {
        Ok(request) => dispatch(app, &request),
        Err(message) => Response::error(400, message),
    };
    let headers: Map<String, Value> = response
        .headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), Value::String(value.clone())))
        .filter(|(name, _)| !DROPPED_HEADERS.contains(&name.as_str()))
        .collect();
    for frame in res_frames(&rid, response.status, headers, response.bytes()) {
        let _ = outbox.send(frame);
    }
}

/// The hub's answer to `request` as the network link gives it: the owner-token gate, then the routes.
fn dispatch(app: &App, request: &Request) -> Response {
    match crate::hub_auth::gate(app, request) {
        Some(refused) => refused,
        None => crate::api::router().handle(app, request),
    }
}

/// The request a `req` frame describes, for `/artifact/{id}{path}?{query}`. A share id is lower-case letters and
/// digits, and a path with a `..` segment or a query is refused. The `x-apz-relay` header carries the relay marker,
/// which marks the request as relayed, and a `/` path is the share's root (`/artifact/{id}`). The peer is the
/// documentation address 192.0.2.1, never loopback, so the owner gate treats a relayed request as remote. Any
/// `x-apz-via` or `x-apz-relay` header the client sent is dropped first.
fn request_of(msg: &Value) -> Result<Request, &'static str> {
    let id = msg.get("id").and_then(Value::as_str).unwrap_or_default();
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        return Err("the share id is not valid");
    }
    let path = msg.get("path").and_then(Value::as_str).unwrap_or("/");
    if !path.starts_with('/')
        || path.contains('?')
        || path.split('/').any(|segment| segment == "..")
    {
        return Err("the path is not valid");
    }
    let suffix = if path == "/" { "" } else { path };
    let mut headers: Vec<(String, String)> = msg
        .get("headers")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(name, value)| {
                    value
                        .as_str()
                        .map(|value| (name.to_ascii_lowercase(), value.to_string()))
                })
                .filter(|(name, _)| name != "x-apz-via" && name != "x-apz-relay")
                .collect()
        })
        .unwrap_or_default();
    headers.push(("x-apz-relay".to_string(), relay_marker().to_string()));
    let body = match msg.get("body").and_then(Value::as_str) {
        None | Some("") => Vec::new(),
        Some(text) => STANDARD
            .decode(text)
            .map_err(|_| "the body is not base64")?,
    };
    Ok(Request {
        method: msg
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("GET")
            .to_ascii_uppercase(),
        path: format!("/artifact/{id}{suffix}"),
        query: msg
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        headers,
        body,
        // 192.0.2.1 is a documentation address, so the owner gate never takes a relayed request for loopback.
        peer: Some(std::net::IpAddr::from([192, 0, 2, 1])),
    })
}

/// The `res` frames of one answer. The body is split into base64 chunks of at most `CHUNK_BYTES`, and the last chunk
/// has `more` false. Every frame carries the request id, the status and the headers, so the first one is enough for
/// the relay.
fn res_frames(rid: &Value, status: u16, headers: Map<String, Value>, body: &[u8]) -> Vec<String> {
    let chunks: Vec<&[u8]> = if body.is_empty() {
        vec![body]
    } else {
        body.chunks(CHUNK_BYTES).collect()
    };
    let last = chunks.len() - 1;
    chunks
        .iter()
        .enumerate()
        .map(|(index, chunk)| {
            json!({
                "t": "res",
                "rid": rid,
                "status": status,
                "headers": Value::Object(headers.clone()),
                "body": STANDARD.encode(chunk),
                "more": index < last,
            })
            .to_string()
        })
        .collect()
}

/// Files a `share.reported` approval for the owner. The reason is kept to 500 characters.
fn report(app: &App, msg: &Value) {
    let id = msg.get("id").and_then(Value::as_str).unwrap_or_default();
    let reason: String = msg
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .chars()
        .take(500)
        .collect();
    approvals::submit(
        app,
        "share.reported",
        id,
        json!({"share": id, "reason": reason}),
    );
}

/// The wait before reconnect number `attempt` (0 for the first): 1 s, doubling up to 60 s, times a jitter of 20 %
/// either way. `unit` in [0, 1] picks the jitter, 0 giving 0.8 and 1 giving 1.2.
fn backoff_delay(attempt: u32, unit: f64) -> Duration {
    let base = 2f64.powi(attempt.min(6) as i32).min(BACKOFF_MAX_SECS);
    let jitter = 0.8 + 0.4 * unit.clamp(0.0, 1.0);
    Duration::from_secs_f64(base * jitter)
}

/// The wait before reconnect number `attempt`, with a random jitter (see `backoff_delay`).
fn backoff(attempt: u32) -> Duration {
    backoff_delay(attempt, jitter_unit())
}

/// A random number in [0, 1] from the OS, or 0.5 when the OS has none.
fn jitter_unit() -> f64 {
    let mut bytes = [0u8; 8];
    match apz_os::random::fill(&mut bytes) {
        Ok(()) => u64::from_le_bytes(bytes) as f64 / u64::MAX as f64,
        Err(_) => 0.5,
    }
}

/// The hub's relay key from `path`: 32 bytes. A missing or malformed file is replaced by 32 new random bytes, written
/// with mode 0600 on Unix.
fn load_key(path: &Path) -> std::io::Result<SigningKey> {
    if let Ok(bytes) = std::fs::read(path)
        && let Ok(secret) = <[u8; 32]>::try_from(bytes.as_slice())
    {
        return Ok(SigningKey::from_bytes(&secret));
    }
    let mut secret = [0u8; 32];
    apz_os::random::fill(&mut secret)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(&secret)?;
    Ok(SigningKey::from_bytes(&secret))
}

/// The outbox of a relay, which ignores a poisoned lock: the frames it holds are still valid.
fn lock(outbox: &Mutex<Option<Sender<String>>>) -> MutexGuard<'_, Option<Sender<String>>> {
    outbox.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The current time in Unix seconds (0 before the epoch).
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, Verifier};

    /// A fixed key for the tests. It is never used on the network.
    fn key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    #[test]
    fn signed_verifies_over_the_exact_bytes() {
        let key = key();
        let sig = signed(&key, "claim", &["hub1", "share1", "1700000000"]);
        assert!(!sig.contains('=') && !sig.contains('+') && !sig.contains('/'));
        let bytes = URL_SAFE_NO_PAD.decode(&sig).expect("base64url");
        let signature = Signature::from_slice(&bytes).expect("64 bytes");
        let public = key.verifying_key();
        assert!(
            public
                .verify(b"apz-relay-claim\nhub1\nshare1\n1700000000", &signature)
                .is_ok()
        );
        assert!(
            public
                .verify(b"apz-relay-claim\nhub1\nshare1\n1700000001", &signature)
                .is_err()
        );
        assert!(
            public
                .verify(b"apz-relay-release\nhub1\nshare1\n1700000000", &signature)
                .is_err()
        );
    }

    #[test]
    fn the_relay_is_dialled_at_the_bases_origin() {
        assert_eq!(origin("https://agentplaza.io"), "https://agentplaza.io");
        assert_eq!(origin("https://agentplaza.io/"), "https://agentplaza.io");
        assert_eq!(
            origin("http://127.0.0.1:7790/r?x=1"),
            "http://127.0.0.1:7790"
        );
        let (tls, host, port) =
            apz_http::client::split_base(&origin("https://agentplaza.io/")).unwrap();
        assert_eq!((tls, host.as_str(), port), (true, "agentplaza.io", 443));
    }

    #[test]
    fn hub_id_is_the_first_half_of_the_key_hash() {
        let public = key().verifying_key().to_bytes();
        let expected: String = Sha256::digest(public)
            .iter()
            .take(16)
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let id = hub_id(&public);
        assert_eq!(id.len(), 32);
        assert_eq!(id, expected);
    }

    #[test]
    fn claim_frame_has_the_protocol_fields() {
        let key = key();
        let hub = hub_id(&key.verifying_key().to_bytes());
        let frame: Value =
            serde_json::from_str(&share_frame(&key, &hub, "claim", "abc", 1_700_000_000))
                .expect("JSON");
        assert_eq!(frame["t"], "claim");
        assert_eq!(frame["id"], "abc");
        assert_eq!(frame["ts"], 1_700_000_000);
        let sig = frame["sig"].as_str().expect("sig");
        assert_eq!(
            sig,
            signed(&key, "claim", &[hub.as_str(), "abc", "1700000000"])
        );
    }

    #[test]
    fn a_large_body_goes_in_512_kib_frames() {
        let body = vec![0xABu8; (1.2 * 1024.0 * 1024.0) as usize];
        let frames: Vec<Value> = res_frames(&json!("r1"), 200, Map::new(), &body)
            .iter()
            .map(|frame| serde_json::from_str(frame).expect("JSON"))
            .collect();
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0]["more"], true);
        assert_eq!(frames[1]["more"], true);
        assert_eq!(frames[2]["more"], false);
        assert_eq!(frames[2]["status"], 200);
        assert_eq!(frames[0]["rid"], "r1");
        let mut joined = Vec::new();
        for frame in &frames {
            joined.extend(
                STANDARD
                    .decode(frame["body"].as_str().expect("body"))
                    .expect("base64"),
            );
        }
        assert_eq!(joined, body);
    }

    #[test]
    fn an_empty_body_is_one_final_frame() {
        let frames = res_frames(&json!("r2"), 404, Map::new(), &[]);
        assert_eq!(frames.len(), 1);
        let frame: Value = serde_json::from_str(&frames[0]).expect("JSON");
        assert_eq!(frame["more"], false);
        assert_eq!(frame["body"], "");
    }

    #[test]
    fn a_relayed_request_maps_onto_the_artifact_path() {
        let msg = json!({
            "t": "req", "rid": "r", "id": "0123456789abcdef", "method": "get",
            "path": "/", "query": "v=2", "headers": {"Accept": "text/html", "X-Apz-Relay": "forged"},
            "body": ""
        });
        let request = request_of(&msg).expect("a valid request");
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/artifact/0123456789abcdef");
        assert_eq!(request.query, "v=2");
        assert!(
            request
                .headers
                .contains(&("x-apz-relay".to_string(), relay_marker().to_string()))
        );
        assert!(
            !request.headers.iter().any(|(_, value)| value == "forged"),
            "a client's own x-apz-relay header is dropped"
        );
        assert_eq!(request.peer, Some(std::net::IpAddr::from([192, 0, 2, 1])));
        assert!(
            request
                .headers
                .contains(&("accept".to_string(), "text/html".to_string()))
        );
        let escape = json!({"id": "0123456789abcdef", "path": "/../v1/shares"});
        assert!(request_of(&escape).is_err());
    }

    #[test]
    fn backoff_doubles_to_a_minute_within_the_jitter() {
        let bases = [1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 60.0, 60.0, 60.0];
        for (attempt, base) in bases.iter().enumerate() {
            for unit in [0.0, 0.5, 1.0] {
                let secs = backoff_delay(attempt as u32, unit).as_secs_f64();
                assert!(
                    secs >= base * 0.8 - 1e-9 && secs <= base * 1.2 + 1e-9,
                    "attempt {attempt}: {secs}"
                );
            }
        }
        assert_eq!(backoff_delay(0, 0.5), Duration::from_secs(1));
        assert_eq!(backoff_delay(40, 0.5), Duration::from_secs(60));
    }
}
