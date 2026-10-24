//! The glue tier (PLAN 14.4): a connector whose code needs a secret runs on the hub, not in a fork, as a WebAssembly
//! component under wasmtime's component model.
//!
//! The component imports three functions and exports `run`:
//! - `fetch(request: string) -> string`: the request is JSON `{"method", "url", "headers"?, "body"?}`. The hub checks
//!   the URL is `https://` to a host in the connector's approved egress, refuses the methods and headers it does not
//!   allow, adds the secret's header (the hub's value, never handed to the component), sends it, and answers
//!   `{"status", "body"}` or `{"error"}`. A secret value that an API echoes back is redacted in the reply.
//! - `kv-get(key) -> string` and `kv-set(key, value) -> string`: the connector's key-value state, which the hub keeps
//!   (the component keeps nothing else between calls). `kv-set` answers "" or the refusal.
//! - `run(input: string) -> string`: one call. The input and the output are the component's own data.
//!
//! Each call has a wall-clock limit (an epoch deadline, ticked by one thread for the process) and a memory limit.
//! Components are compiled from their binary form; WAT is accepted only in tests (the `wat` feature). Nothing is
//! reachable from a component but these imports: no WASI, no files, no clock.

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store, StoreContextMut, StoreLimitsBuilder};

/// The wall-clock limit of one call, host calls included, unless a [`Host`] says otherwise.
pub const TIME_LIMIT: Duration = Duration::from_secs(5);
/// The most linear memory one call may hold.
pub const MEMORY_LIMIT: usize = 32 << 20;
/// The granularity of the time limit: the epoch's tick.
const TICK: Duration = Duration::from_millis(10);
/// The timeout of one request the hub sends.
const TIMEOUT: Duration = Duration::from_secs(30);
/// A request's body and a reply's body are cut at this size (as `connectors.rs` cuts a reply).
pub const BODY_CAP: usize = 256 * 1024;
/// The longest key and the largest value a component may store.
const KEY_CAP: usize = 128;
const VALUE_CAP: usize = 64 * 1024;
/// Headers a component may not set: hop-by-hop, framing and routing ones (as `egress_inject.rs` refuses).
const REFUSED_HEADERS: [&str; 10] = [
    "connection",
    "keep-alive",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
];
const METHODS: [&str; 5] = ["GET", "POST", "PUT", "PATCH", "DELETE"];

/// A request the hub sends, as the component asked for it, plus the hub's own header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// What an upstream answered: the status, and the body cut at 256 KiB. `bytes` is the body as it came, cut at
/// [`ARTIFACT_CAP`] plus one byte (so a caller can tell that it was larger): an artifact is binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub bytes: Vec<u8>,
}

/// The most of an upstream body the reply keeps as bytes: a device farm's artifact (a video or a log) is downloaded whole up
/// to this size, and a larger one is refused by its caller.
pub const ARTIFACT_CAP: usize = 16 << 20;

/// The way the hub reaches the network. Production uses [`HttpsOutbound`]; tests put a fake server behind it.
pub trait Outbound: Send + Sync {
    fn send(&self, request: &Outgoing) -> Result<Reply, String>;
}

/// A transport for a test hub: each request goes to the fake API at `addr` with the same path and headers, so the glue
/// tier's checks and the secret's header are the hub's own. A hub uses it when `APZ_GLUE_STAND_IN` names an address
/// (`tool_life.rs`); production never sets it.
pub struct StandIn {
    pub addr: String,
}

impl Outbound for StandIn {
    fn send(&self, request: &Outgoing) -> Result<Reply, String> {
        let path = request
            .url
            .strip_prefix("https://")
            .and_then(|rest| rest.find('/').map(|i| &rest[i..]))
            .unwrap_or("/");
        HttpsOutbound.send(&Outgoing {
            url: format!("http://{}{path}", self.addr),
            ..request.clone()
        })
    }
}

/// The secret a connector's requests carry: the header and its value, already formatted by the hub. Never shown to
/// the component, and redacted from anything the component reads back.
#[derive(Clone)]
pub struct Secret {
    pub header: String,
    pub value: String,
}

/// What one call runs with: the approved egress hosts, the secret, the transport, the connector's key-value state,
/// and the limits.
pub struct Host {
    pub egress: Vec<String>,
    pub secret: Option<Secret>,
    pub outbound: Arc<dyn Outbound>,
    pub kv: BTreeMap<String, String>,
    pub time_limit: Duration,
    pub memory_limit: usize,
}

impl Host {
    /// A host with the default limits.
    pub fn new(
        egress: Vec<String>,
        secret: Option<Secret>,
        outbound: Arc<dyn Outbound>,
        kv: BTreeMap<String, String>,
    ) -> Self {
        Host {
            egress,
            secret,
            outbound,
            kv,
            time_limit: TIME_LIMIT,
            memory_limit: MEMORY_LIMIT,
        }
    }
}

/// What a call returned.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub output: String,
    /// The key-value entries the call wrote, in order: the hub stores them.
    pub kv_changes: Vec<(String, String)>,
    /// Everything that crossed into or out of the component, in order. Never a secret's value.
    pub crossings: Vec<String>,
}

/// The production transport: one HTTPS request by ureq, the reply's body cut at 256 KiB.
pub struct HttpsOutbound;

impl Outbound for HttpsOutbound {
    fn send(&self, request: &Outgoing) -> Result<Reply, String> {
        let mut builder = ureq::http::Request::builder()
            .method(request.method.as_str())
            .uri(request.url.as_str());
        for (name, value) in &request.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let http = builder
            .body(request.body.clone())
            .map_err(|e| format!("bad request: {e}"))?;
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .http_status_as_error(false)
            .build();
        let agent = ureq::Agent::new_with_config(config);
        let response = agent
            .run(http)
            .map_err(|e| format!("the request failed: {e}"))?;
        let status = response.status().as_u16();
        let mut bytes = Vec::new();
        response
            .into_body()
            .into_reader()
            .take(ARTIFACT_CAP as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let text = &bytes[..bytes.len().min(BODY_CAP)];
        Ok(Reply {
            status,
            body: String::from_utf8_lossy(text).into_owned(),
            bytes,
        })
    }
}

/// The request a component writes to `fetch`.
#[derive(Deserialize)]
struct Request {
    method: String,
    url: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    body: String,
}

/// The state of one call: what the host functions read and write.
struct State {
    limits: wasmtime::StoreLimits,
    egress: Vec<String>,
    secret: Option<Secret>,
    outbound: Arc<dyn Outbound>,
    kv: BTreeMap<String, String>,
    kv_changes: Vec<(String, String)>,
    crossings: Vec<String>,
}

static ENGINE: OnceLock<Result<Engine, String>> = OnceLock::new();

/// The process's engine. One thread ticks its epoch, so a call's deadline is a count of ticks.
fn engine() -> Result<&'static Engine, String> {
    ENGINE
        .get_or_init(|| {
            let mut config = Config::new();
            config.epoch_interruption(true);
            let engine = Engine::new(&config).map_err(|e| e.to_string())?;
            let ticking = engine.clone();
            std::thread::Builder::new()
                .name("glue-epoch".into())
                .spawn(move || {
                    loop {
                        std::thread::sleep(TICK);
                        ticking.increment_epoch();
                    }
                })
                .map_err(|e| e.to_string())?;
            Ok(engine)
        })
        .as_ref()
        .map_err(Clone::clone)
}

/// Runs `export` of `component` on `input` under `host`. A component that traps, runs past its time limit or
/// grows past its memory limit is an error, and its partial effects stay as they were sent.
pub fn run(component: &[u8], export: &str, input: &str, host: Host) -> Result<Outcome, String> {
    let engine = engine()?;
    let component =
        Component::new(engine, component).map_err(|e| format!("not a component: {e:?}"))?;
    let mut linker: Linker<State> = Linker::new(engine);
    {
        let mut root = linker.root();
        root.func_wrap(
            "fetch",
            |mut cx: StoreContextMut<'_, State>,
             (request,): (String,)|
             -> wasmtime::Result<(String,)> { Ok((fetch(cx.data_mut(), &request),)) },
        )
        .map_err(|e| e.to_string())?;
        root.func_wrap(
            "kv-get",
            |mut cx: StoreContextMut<'_, State>,
             (key,): (String,)|
             -> wasmtime::Result<(String,)> { Ok((kv_get(cx.data_mut(), &key),)) },
        )
        .map_err(|e| e.to_string())?;
        root.func_wrap(
            "kv-set",
            |mut cx: StoreContextMut<'_, State>,
             (key, value): (String, String)|
             -> wasmtime::Result<(String,)> {
                Ok((kv_set(cx.data_mut(), &key, &value),))
            },
        )
        .map_err(|e| e.to_string())?;
    }

    let state = State {
        limits: StoreLimitsBuilder::new()
            .memory_size(host.memory_limit)
            .build(),
        egress: host.egress,
        secret: host.secret,
        outbound: host.outbound,
        kv: host.kv,
        kv_changes: Vec::new(),
        crossings: vec![format!("input {input}")],
    };
    let mut store = Store::new(engine, state);
    store.limiter(|s: &mut State| -> &mut dyn wasmtime::ResourceLimiter { &mut s.limits });
    store.set_epoch_deadline(ticks(host.time_limit));

    let instance = linker
        .instantiate(&mut store, &component)
        .map_err(|e| format!("the component did not start: {e}"))?;
    let call = instance
        .get_typed_func::<(String,), (String,)>(&mut store, export)
        .map_err(|e| format!("no export `{export}`: {e}"))?;
    let (output,) = call
        .call(&mut store, (input.to_string(),))
        .map_err(|e| format!("`{export}` stopped: {e}"))?;
    let mut state = store.into_data();
    state.crossings.push(format!("output {output}"));
    Ok(Outcome {
        output,
        kv_changes: state.kv_changes,
        crossings: state.crossings,
    })
}

/// Checks that `component` is a WebAssembly component that exports `run`, without running it: a proposal's check.
pub fn check_component(component: &[u8]) -> Result<(), String> {
    let engine = engine()?;
    let parsed =
        Component::new(engine, component).map_err(|e| format!("not a component: {e:?}"))?;
    if parsed.get_export_index(None, "run").is_none() {
        return Err("the component exports no `run`".to_string());
    }
    Ok(())
}

/// The epoch ticks a time limit allows.
fn ticks(limit: Duration) -> u64 {
    (limit.as_millis() / TICK.as_millis()).max(1) as u64
}

/// `fetch`: the reply the component reads, as JSON text. A refusal or a failure is `{"error"}`, never a trap.
fn fetch(state: &mut State, request: &str) -> String {
    state.crossings.push(format!("request {request}"));
    let text = match send(state, request) {
        Ok(reply) => json!({ "status": reply.status, "body": redact(state, &reply.body) }),
        Err(error) => json!({ "error": redact(state, &error) }),
    }
    .to_string();
    state.crossings.push(format!("reply {text}"));
    text
}

/// The hub's checks on one request, then the send with the secret's header added.
fn send(state: &State, request: &str) -> Result<Reply, String> {
    let req: Request =
        serde_json::from_str(request).map_err(|e| format!("the request is not JSON: {e}"))?;
    Mediator {
        egress: &state.egress,
        secret: state.secret.as_ref(),
        outbound: &*state.outbound,
        loopback_http: false,
        body_cap: BODY_CAP,
    }
    .send(&req.method, &req.url, &req.headers, &req.body)
}

/// The hub's way out for one request: the checks on the URL, the method and the headers, the secret's header added,
/// the send. The glue tier's `fetch` goes through it, and so do the intake connectors' calls (`intake`), whose
/// secrets are the same kind: never handed to anything that runs code. `loopback_http` lets a request go to a
/// service on this machine over plain http (a stand-in for a service in the tests, or a local one the owner runs);
/// the glue tier never does. `body_cap` is the most a request body may be: the glue tier's 256 KiB, or a publish's
/// (a package's own bytes, 9.5).
pub struct Mediator<'a> {
    pub egress: &'a [String],
    pub secret: Option<&'a Secret>,
    pub outbound: &'a dyn Outbound,
    pub loopback_http: bool,
    pub body_cap: usize,
}

impl Mediator<'_> {
    /// Sends `method` `url` with `headers` and `body` when the checks pass; the reply's body is redacted of the secret.
    pub fn send(
        &self,
        method: &str,
        url: &str,
        headers: &BTreeMap<String, String>,
        body: &str,
    ) -> Result<Reply, String> {
        self.send_bytes(method, url, headers, body.as_bytes())
    }

    /// [`Mediator::send`] for a body that is bytes (a package's file, a registry's blob).
    pub fn send_bytes(
        &self,
        method: &str,
        url: &str,
        headers: &BTreeMap<String, String>,
        body: &[u8],
    ) -> Result<Reply, String> {
        let (https, rest) = match url.strip_prefix("https://") {
            Some(rest) => (true, rest),
            None => match url.strip_prefix("http://") {
                Some(rest) if self.loopback_http => (false, rest),
                _ if self.loopback_http => {
                    return Err(
                        "only https:// URLs, or http:// to this machine, can be requested".into(),
                    );
                }
                _ => return Err("only https:// URLs can be requested".into()),
            },
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        if authority.contains('@') {
            return Err("a URL may not carry credentials".into());
        }
        let host = if https {
            match authority.strip_suffix(":443") {
                Some(host) => host,
                None if authority.contains(':') => {
                    return Err("only the default port may be used".into());
                }
                None => authority,
            }
        } else {
            authority.split(':').next().unwrap_or("")
        }
        .to_ascii_lowercase();
        let loopback = matches!(host.as_str(), "127.0.0.1" | "localhost");
        if https {
            if !self.egress.contains(&host) {
                return Err(format!(
                    "egress: `{host}` is not a host this connector may reach"
                ));
            }
        } else if !loopback {
            return Err(format!(
                "egress: `{host}` is not this machine, so it needs https"
            ));
        }
        let method = method.to_ascii_uppercase();
        if !METHODS.contains(&method.as_str()) {
            return Err(format!("method `{method}` is not allowed"));
        }
        let auth_header = self.secret.map(|s| s.header.to_ascii_lowercase());
        let mut out_headers: Vec<(String, String)> = Vec::new();
        for (name, value) in headers {
            let lower = name.to_ascii_lowercase();
            if REFUSED_HEADERS.contains(&lower.as_str())
                || auth_header.as_deref() == Some(lower.as_str())
            {
                continue;
            }
            out_headers.push((name.clone(), value.clone()));
        }
        if let Some(secret) = self.secret {
            out_headers.push((secret.header.clone(), secret.value.clone()));
        }
        if body.len() > self.body_cap {
            return Err(format!("a request body is at most {} bytes", self.body_cap));
        }
        let reply = self.outbound.send(&Outgoing {
            method,
            url: url.to_string(),
            headers: out_headers,
            body: body.to_vec(),
        })?;
        Ok(Reply {
            status: reply.status,
            bytes: reply.bytes,
            body: match self.secret {
                Some(secret) if !secret.value.is_empty() => {
                    reply.body.replace(&secret.value, "[redacted]")
                }
                _ => reply.body,
            },
        })
    }
}

/// Replaces the secret's value wherever it appears, so a reply that echoes it reaches the component without it.
fn redact(state: &State, text: &str) -> String {
    match &state.secret {
        Some(secret) if !secret.value.is_empty() => text.replace(&secret.value, "[redacted]"),
        _ => text.to_string(),
    }
}

/// `kv-get`: the value under `key`, or "".
fn kv_get(state: &mut State, key: &str) -> String {
    state.crossings.push(format!("kv-get {key}"));
    state.kv.get(key).cloned().unwrap_or_default()
}

/// `kv-set`: stores `value` under `key`; "" when stored, else the refusal.
fn kv_set(state: &mut State, key: &str, value: &str) -> String {
    state.crossings.push(format!("kv-set {key}"));
    if key.is_empty() || key.len() > KEY_CAP {
        return format!("a key is 1 to {KEY_CAP} bytes");
    }
    if value.len() > VALUE_CAP {
        return "a value is at most 64 KiB".into();
    }
    state.kv.insert(key.to_string(), value.to_string());
    state.kv_changes.push((key.to_string(), value.to_string()));
    String::new()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Mutex;
    use std::time::Instant;

    const SECRET: &str = "Bearer SENTINEL-GLUE-7731";

    /// A component that forwards its input to `fetch` and returns the reply; `spin` never returns; `grow` asks for
    /// more memory than its limit; `kv` stores its input under itself and reads it back. (Shared with connectors.rs'
    /// tests.)
    pub(crate) const COMPONENT: &str = r#"
    (component
      (import "fetch" (func $host-fetch (param "request" string) (result string)))
      (import "kv-get" (func $host-kv-get (param "key" string) (result string)))
      (import "kv-set" (func $host-kv-set (param "key" string) (param "value" string) (result string)))

      (core module $libc
        (memory (export "memory") 2)
        (global $next (mut i32) (i32.const 8192))
        (func (export "realloc") (param $old i32) (param $oldsize i32) (param $align i32) (param $newsize i32) (result i32)
          (local $p i32)
          (local.set $p (global.get $next))
          (global.set $next (i32.add (global.get $next) (local.get $newsize)))
          (local.get $p)))
      (core instance $libc-i (instantiate $libc))

      (core func $fetch-low
        (canon lower (func $host-fetch) (memory (core memory $libc-i "memory")) (realloc (core func $libc-i "realloc"))))
      (core func $kv-get-low
        (canon lower (func $host-kv-get) (memory (core memory $libc-i "memory")) (realloc (core func $libc-i "realloc"))))
      (core func $kv-set-low
        (canon lower (func $host-kv-set) (memory (core memory $libc-i "memory")) (realloc (core func $libc-i "realloc"))))
      (core instance $host
        (export "fetch" (func $fetch-low))
        (export "kv-get" (func $kv-get-low))
        (export "kv-set" (func $kv-set-low)))

      (core module $guest
        (import "host" "fetch" (func $fetch (param i32 i32 i32)))
        (import "host" "kv-get" (func $kv-get (param i32 i32 i32)))
        (import "host" "kv-set" (func $kv-set (param i32 i32 i32 i32 i32)))
        (import "libc" "memory" (memory 2))
        ;; A lifted string result: the export returns the address of its (ptr, len) pair, in scratch cells
        ;; below the allocator's heap (16: kv-set's reply, 32: fetch's and grow's pair, 40: kv-get's pair).
        (func (export "run") (param $ptr i32) (param $len i32) (result i32)
          (call $fetch (local.get $ptr) (local.get $len) (i32.const 32))
          (i32.const 32))
        (func (export "spin") (param i32 i32) (result i32)
          (loop $again (br $again))
          (i32.const 0))
        (func (export "grow") (param i32 i32) (result i32)
          (if (i32.eq (memory.grow (i32.const 1000)) (i32.const -1))
            (then unreachable))
          (i32.store (i32.const 32) (i32.const 0))
          (i32.store offset=4 (i32.const 32) (i32.const 0))
          (i32.const 32))
        (func (export "kv") (param $ptr i32) (param $len i32) (result i32)
          (call $kv-set (local.get $ptr) (local.get $len) (local.get $ptr) (local.get $len) (i32.const 16))
          (call $kv-get (local.get $ptr) (local.get $len) (i32.const 40))
          (i32.const 40)))
      (core instance $guest
        (instantiate $guest
          (with "host" (instance $host))
          (with "libc" (instance $libc-i))))

      (type $sig (func (param "input" string) (result string)))
      (func $run (type $sig)
        (canon lift (core func $guest "run") (memory (core memory $libc-i "memory")) (realloc (core func $libc-i "realloc"))))
      (func $spin (type $sig)
        (canon lift (core func $guest "spin") (memory (core memory $libc-i "memory")) (realloc (core func $libc-i "realloc"))))
      (func $grow (type $sig)
        (canon lift (core func $guest "grow") (memory (core memory $libc-i "memory")) (realloc (core func $libc-i "realloc"))))
      (func $kv (type $sig)
        (canon lift (core func $guest "kv") (memory (core memory $libc-i "memory")) (realloc (core func $libc-i "realloc"))))
      (export "run" (func $run))
      (export "spin" (func $spin))
      (export "grow" (func $grow))
      (export "kv" (func $kv)))
    "#;

    /// The test component as a binary: the WAT text, assembled.
    pub(crate) fn component() -> Vec<u8> {
        wat::parse_str(COMPONENT).expect("the test component is valid WAT")
    }

    /// A fake API on 127.0.0.1: it answers every request with `body` and records each request's head.
    struct FakeApi {
        port: u16,
        heads: Arc<Mutex<Vec<String>>>,
    }

    fn fake_api(body: &'static str) -> FakeApi {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let heads = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&heads);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if stream.read_exact(&mut byte).is_err() {
                        break;
                    }
                    head.push(byte[0]);
                }
                log.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&head).into_owned());
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        FakeApi { port, heads }
    }

    /// The test transport: `https://<host>/…` goes to the fake API on 127.0.0.1, and every request it is asked to
    /// send is kept.
    struct Redirect {
        port: u16,
        sent: Mutex<Vec<Outgoing>>,
    }

    impl Outbound for Redirect {
        fn send(&self, request: &Outgoing) -> Result<Reply, String> {
            self.sent.lock().unwrap().push(request.clone());
            let path = request
                .url
                .strip_prefix("https://")
                .and_then(|rest| rest.find('/').map(|i| &rest[i..]))
                .unwrap_or("/");
            HttpsOutbound.send(&Outgoing {
                url: format!("http://127.0.0.1:{}{path}", self.port),
                ..request.clone()
            })
        }
    }

    fn redirect(api: &FakeApi) -> Arc<Redirect> {
        Arc::new(Redirect {
            port: api.port,
            sent: Mutex::new(Vec::new()),
        })
    }

    fn host(outbound: Arc<dyn Outbound>, secret: Option<&str>) -> Host {
        Host::new(
            vec!["api.acme-test.example".into()],
            secret.map(|value| Secret {
                header: "Authorization".into(),
                value: value.into(),
            }),
            outbound,
            BTreeMap::new(),
        )
    }

    #[test]
    fn a_component_calls_its_api_and_the_secret_stays_in_the_hub() {
        let api = fake_api(r#"{"notes":["first"]}"#);
        let outbound = redirect(&api);
        let input = r#"{"method":"GET","url":"https://api.acme-test.example/v1/notes?q=glue"}"#;
        let out = run(
            &component(),
            "run",
            input,
            host(outbound.clone(), Some(SECRET)),
        )
        .unwrap();

        assert!(out.output.contains(r#""status":200"#), "{}", out.output);
        assert!(out.output.contains("first"), "{}", out.output);
        // The API received the secret's header, on the request the hub sent.
        let heads = api.heads.lock().unwrap();
        assert_eq!(heads.len(), 1);
        assert!(
            heads[0].starts_with("GET /v1/notes?q=glue "),
            "{}",
            heads[0]
        );
        assert!(
            heads[0]
                .to_ascii_lowercase()
                .contains(&format!("authorization: {}", SECRET.to_ascii_lowercase())),
            "{}",
            heads[0]
        );
        assert_eq!(
            outbound.sent.lock().unwrap()[0].url,
            "https://api.acme-test.example/v1/notes?q=glue"
        );
        // The component never held it: not in its input, its output, or anything that crossed.
        assert!(!input.contains("SENTINEL"));
        assert!(!out.output.contains("SENTINEL"));
        for line in &out.crossings {
            assert!(!line.contains("SENTINEL"), "crossed: {line}");
        }
    }

    #[test]
    fn a_secret_an_api_echoes_back_is_redacted_before_the_component_reads_it() {
        let api = fake_api(r#"{"you_sent":"Bearer SENTINEL-GLUE-7731"}"#);
        let input = r#"{"method":"GET","url":"https://api.acme-test.example/v1/me"}"#;
        let out = run(
            &component(),
            "run",
            input,
            host(redirect(&api), Some(SECRET)),
        )
        .unwrap();
        assert!(out.output.contains("[redacted]"), "{}", out.output);
        assert!(!out.output.contains("SENTINEL"));
    }

    #[test]
    fn a_host_outside_the_egress_is_refused_and_nothing_is_sent() {
        let api = fake_api("{}");
        let outbound = redirect(&api);
        for url in [
            "https://evil.example.net/steal",
            "http://api.acme-test.example/v1/notes",
            "https://api.acme-test.example:8443/v1/notes",
            "https://api.acme-test.example@evil.example/v1/notes",
            "https://API.ACME-TEST.EXAMPLE.evil.example/x",
        ] {
            let input = json!({ "method": "GET", "url": url }).to_string();
            let out = run(
                &component(),
                "run",
                &input,
                host(outbound.clone(), Some(SECRET)),
            )
            .unwrap();
            assert!(out.output.contains(r#""error""#), "{url}: {}", out.output);
            assert!(!out.output.contains("SENTINEL"), "{url}");
        }
        assert!(outbound.sent.lock().unwrap().is_empty());
        assert!(api.heads.lock().unwrap().is_empty());
    }

    #[test]
    fn the_component_cannot_set_the_secrets_header_itself() {
        let api = fake_api("{}");
        let input = json!({
            "method": "GET",
            "url": "https://api.acme-test.example/v1/x",
            "headers": { "authorization": "Bearer forged", "host": "evil.example", "x-trace": "1" }
        })
        .to_string();
        run(
            &component(),
            "run",
            &input,
            host(redirect(&api), Some(SECRET)),
        )
        .unwrap();
        let heads = api.heads.lock().unwrap();
        assert!(
            heads[0]
                .to_ascii_lowercase()
                .contains(&format!("authorization: {}", SECRET.to_ascii_lowercase())),
            "{}",
            heads[0]
        );
        assert!(!heads[0].contains("forged"), "{}", heads[0]);
        assert!(heads[0].contains("x-trace: 1"), "{}", heads[0]);
    }

    #[test]
    fn a_looping_component_is_stopped_by_the_time_limit() {
        let mut slow = host(redirect(&fake_api("{}")), None);
        slow.time_limit = Duration::from_millis(300);
        let started = Instant::now();
        let err = run(&component(), "spin", "{}", slow).unwrap_err();
        assert!(err.contains("spin"), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_component_past_its_memory_limit_is_stopped() {
        let err = run(
            &component(),
            "grow",
            "{}",
            host(redirect(&fake_api("{}")), None),
        )
        .unwrap_err();
        assert!(err.contains("grow"), "{err}");
    }

    #[test]
    fn the_key_value_state_is_the_hubs_and_comes_back_as_changes() {
        let mut seeded = host(redirect(&fake_api("{}")), None);
        seeded.kv.insert("other".into(), "kept".into());
        let out = run(&component(), "kv", "cursor-7", seeded).unwrap();
        assert_eq!(out.output, "cursor-7");
        assert_eq!(
            out.kv_changes,
            vec![("cursor-7".to_string(), "cursor-7".to_string())]
        );
    }

    #[test]
    fn bytes_that_are_not_a_component_and_unknown_exports_are_errors() {
        let outbound = redirect(&fake_api("{}"));
        assert!(
            run(
                b"\0not a component",
                "run",
                "{}",
                host(outbound.clone(), None)
            )
            .unwrap_err()
            .contains("not a component")
        );
        assert!(
            run(&component(), "absent", "{}", host(outbound, None))
                .unwrap_err()
                .contains("no export")
        );
    }
}
