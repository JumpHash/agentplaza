//! apz-topo-cache: a stand-in cache service for the stage tests (PLAN 22.8), run by `apz_topology::run` in place of an
//! artifact. It is the cache the cache stage judges: a read-through cache over an origin with a TTL, coalescing of
//! concurrent misses on one key, and invalidation on write.
//!
//! It listens on the first port of `APZ_TOPO_PORTS` at `APZ_TOPO_BIND`:
//! - `GET /items/<key>` answers the key's value (empty when none), with `X-Cache: hit` when the cache served it.
//! - `PUT /items/<key>` writes the origin (the request body is the value) and, unless `APZ_CACHE_INVALIDATE=0` is set,
//!   drops the key from the cache. `APZ_CACHE_INVALIDATE=0` is the seeded defect the stage's invalidation check catches.
//! - `GET /origin/requests` answers how many requests the origin has served.
//!
//! Each origin request takes 20 ms, so concurrent misses overlap as they would at a real origin.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// The origin's latency per request.
const ORIGIN_LATENCY: Duration = Duration::from_millis(20);
/// How long a cached value lives.
const TTL: Duration = Duration::from_secs(60);

#[derive(Default)]
struct State {
    origin: Mutex<HashMap<String, String>>,
    origin_hits: AtomicU64,
    cache: Mutex<HashMap<String, (String, Instant)>>,
    flights: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    invalidate: bool,
}

fn main() {
    if let Err(why) = run() {
        eprintln!("apz-topo-cache: {why}");
        std::process::exit(1);
    }
}

fn env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("{name} is not set"))
}

fn run() -> Result<(), String> {
    let bind: Ipv4Addr = env("APZ_TOPO_BIND")?
        .parse()
        .map_err(|e| format!("APZ_TOPO_BIND: {e}"))?;
    let port: u16 = env("APZ_TOPO_PORTS")?
        .split(',')
        .next()
        .unwrap_or_default()
        .parse()
        .map_err(|e| format!("APZ_TOPO_PORTS: {e}"))?;
    let state = Arc::new(State {
        invalidate: std::env::var("APZ_CACHE_INVALIDATE").as_deref() != Ok("0"),
        ..State::default()
    });
    let listener = TcpListener::bind((bind, port)).map_err(|e| format!("{bind}:{port}: {e}"))?;
    for stream in listener.incoming().flatten() {
        let state = Arc::clone(&state);
        thread::spawn(move || {
            let _ = handle(stream, &state);
        });
    }
    Ok(())
}

fn handle(stream: TcpStream, state: &State) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, path) = (
        parts.next().unwrap_or(""),
        parts.next().unwrap_or("").to_string(),
    );
    let mut length = 0_usize;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header)?;
        if header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    let (status, headers, text) = match (method, path.as_str()) {
        ("GET", "/origin/requests") => (
            "200 OK",
            String::new(),
            state.origin_hits.load(Ordering::SeqCst).to_string(),
        ),
        ("GET", p) if p.starts_with("/items/") => {
            let (value, hit) = read(state, &p["/items/".len()..]);
            (
                "200 OK",
                format!("X-Cache: {}\r\n", if hit { "hit" } else { "miss" }),
                value,
            )
        }
        ("PUT", p) if p.starts_with("/items/") => {
            write(
                state,
                &p["/items/".len()..],
                &String::from_utf8_lossy(&body),
            );
            ("200 OK", String::new(), "ok".to_string())
        }
        _ => ("404 Not Found", String::new(), "not found".to_string()),
    };
    let mut stream = stream;
    write!(
        stream,
        "HTTP/1.1 {status}\r\n{headers}Content-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    )?;
    stream.flush()
}

/// The value of `key` and whether the cache served it. A miss goes to the origin, one flight per key at a time: the
/// misses that wait find the value cached.
fn read(state: &State, key: &str) -> (String, bool) {
    if let Some(value) = cached(state, key) {
        return (value, true);
    }
    let flight = match state.flights.lock() {
        Ok(mut flights) => Arc::clone(flights.entry(key.to_string()).or_default()),
        Err(_) => return (fetch(state, key), false),
    };
    let Ok(_turn) = flight.lock() else {
        return (fetch(state, key), false);
    };
    match cached(state, key) {
        Some(value) => (value, true),
        None => (fetch(state, key), false),
    }
}

fn cached(state: &State, key: &str) -> Option<String> {
    let cache = state.cache.lock().ok()?;
    let (value, stored) = cache.get(key)?;
    (stored.elapsed() < TTL).then(|| value.clone())
}

fn fetch(state: &State, key: &str) -> String {
    thread::sleep(ORIGIN_LATENCY);
    state.origin_hits.fetch_add(1, Ordering::SeqCst);
    let value = state
        .origin
        .lock()
        .ok()
        .and_then(|origin| origin.get(key).cloned())
        .unwrap_or_default();
    if let Ok(mut cache) = state.cache.lock() {
        cache.insert(key.to_string(), (value.clone(), Instant::now()));
    }
    value
}

fn write(state: &State, key: &str, value: &str) {
    if let Ok(mut origin) = state.origin.lock() {
        origin.insert(key.to_string(), value.to_string());
    }
    if state.invalidate
        && let Ok(mut cache) = state.cache.lock()
    {
        cache.remove(key);
    }
}
