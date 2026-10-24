//! The cache stage (PLAN 22.8): the hit rate of a service's cache over a profile, its invalidation (a write is read back
//! as the new value within a bound), and a stampede (many concurrent misses on one key, and the origin's request count
//! for them). The stage reaches the cache through a [`Subject`]: the lab's is HTTP ([`Http`]) against the service's
//! declared paths, and the tests' is [`Memory`], an in-process cache with the same contract whose invalidation and
//! coalescing can be switched off.
//!
//! A stampede is set up by writing the key, which invalidates it: the concurrent reads that follow all miss. The origin
//! counts the requests it served; the stage measures how many of them the wave caused.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::compare::Metric;
use crate::stage::{UNREADABLE, not_run, value};

/// The most clients a stampede may declare, and the most keys a profile may draw from.
const MAX_CLIENTS: u32 = 1000;
const MAX_KEYS: u32 = 1_000_000;

/// The cache stage of a topology: the `[cache]` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheStage {
    /// The service whose cache is exercised: a service of the topology, whose URL is named at the run.
    pub service: String,
    /// The GET that reads a key through the cache: a path with `{key}`.
    pub read: String,
    /// The PUT that writes a key at the origin, which invalidates it: a path with `{key}`.
    pub write: String,
    /// The origin's request counter: a GET whose body is the number of requests the origin has served.
    pub origin: String,
    /// The profile: `requests` reads over `keys` keys (80 % of them over the hottest fifth).
    pub keys: u32,
    pub requests: u32,
    /// The hit rate the profile must reach, in percent.
    pub hit_floor_percent: u32,
    /// The bound a write must be read back within, in milliseconds.
    pub invalidation_within_ms: u64,
    /// The concurrent misses of a stampede.
    pub stampede_clients: u32,
    /// The most origin requests the stampede may cause.
    pub stampede_origin_max: u64,
}

impl CacheStage {
    /// What TOML cannot say: the paths are paths with the placeholder, the counts are positive and bounded, and the
    /// floor is a percentage.
    pub fn check(&self) -> Result<(), String> {
        if !crate::spec::is_label(&self.service) {
            return Err(format!(
                "cache service {:?} is not a service name",
                self.service
            ));
        }
        for (what, path) in [
            ("read", &self.read),
            ("write", &self.write),
            ("origin", &self.origin),
        ] {
            if !path.starts_with('/') || path.contains(char::is_whitespace) {
                return Err(format!("cache {what} {path:?} is not a path"));
            }
        }
        for (what, path) in [("read", &self.read), ("write", &self.write)] {
            if !path.contains("{key}") {
                return Err(format!("cache {what} {path:?} has no {{key}}"));
            }
        }
        if self.keys == 0 || self.keys > MAX_KEYS || self.requests == 0 {
            return Err("cache profile needs keys and requests, positive and bounded".into());
        }
        if self.hit_floor_percent > 100 {
            return Err(format!(
                "cache hit floor {} is over 100 %",
                self.hit_floor_percent
            ));
        }
        if self.invalidation_within_ms == 0 {
            return Err("cache invalidation bound is 0 ms".into());
        }
        if self.stampede_clients < 2 || self.stampede_clients > MAX_CLIENTS {
            return Err(format!(
                "cache stampede clients {} is not 2 to {MAX_CLIENTS}",
                self.stampede_clients
            ));
        }
        if self.stampede_origin_max == 0 {
            return Err(
                "cache stampede origin bound is 0 requests (a miss reaches the origin)".into(),
            );
        }
        Ok(())
    }
}

/// The metric names the cache stage records at a run.
pub fn names() -> Vec<String> {
    [
        "cache.profile.requests",
        "cache.profile.hits",
        "cache.profile.errors",
        "cache.profile.hit_percent",
        "cache.invalidation.ms",
        "cache.stampede.origin_requests",
        "cache.stampede.errors",
    ]
    .map(String::from)
    .to_vec()
}

/// The cache under test, as the stage reaches it.
pub trait Subject: Sync {
    /// A read through the cache: the value, and whether the cache served it (not the origin).
    fn read(&self, key: &str) -> Result<(String, bool), String>;
    /// A write at the origin (which invalidates the key in a cache that invalidates).
    fn write(&self, key: &str, value: &str) -> Result<(), String>;
    /// The origin's request count so far.
    fn origin_requests(&self) -> Result<u64, String>;
}

/// The lab's subject: the service's declared paths over HTTP, the cache's hit read from its `x-cache` header.
pub struct Http {
    pub base: String,
    pub read: String,
    pub write: String,
    pub origin: String,
    agent: ureq::Agent,
}

impl Http {
    /// The subject for a service at `base` with the stage's paths.
    pub fn new(base: &str, stage: &CacheStage) -> Http {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(10)))
            .http_status_as_error(false)
            .build();
        Http {
            base: base.trim_end_matches('/').to_string(),
            read: stage.read.clone(),
            write: stage.write.clone(),
            origin: stage.origin.clone(),
            agent: ureq::Agent::new_with_config(config),
        }
    }

    fn url(&self, path: &str, key: &str) -> String {
        format!("{}{}", self.base, path.replace("{key}", key))
    }
}

impl Subject for Http {
    fn read(&self, key: &str) -> Result<(String, bool), String> {
        let url = self.url(&self.read, key);
        let mut response = self
            .agent
            .get(&url)
            .call()
            .map_err(|e| format!("{url}: {e}"))?;
        let status = response.status().as_u16();
        let hit = response
            .headers()
            .get("x-cache")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.eq_ignore_ascii_case("hit"));
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|e| format!("{url}: {e}"))?;
        if status >= 400 {
            return Err(format!("{url} answered {status}"));
        }
        Ok((body, hit))
    }

    fn write(&self, key: &str, value: &str) -> Result<(), String> {
        let url = self.url(&self.write, key);
        let response = self
            .agent
            .put(&url)
            .send(value)
            .map_err(|e| format!("{url}: {e}"))?;
        let status = response.status().as_u16();
        if (200..300).contains(&status) {
            Ok(())
        } else {
            Err(format!("{url} answered {status}"))
        }
    }

    fn origin_requests(&self) -> Result<u64, String> {
        let url = format!("{}{}", self.base, self.origin);
        let mut response = self
            .agent
            .get(&url)
            .call()
            .map_err(|e| format!("{url}: {e}"))?;
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|e| format!("{url}: {e}"))?;
        body.trim()
            .parse()
            .map_err(|_| format!("{url} answered {:?}, not a count", body.trim()))
    }
}

/// An in-process cache with the same contract as a service's: a TTL, invalidation on write (switchable), and coalescing
/// of concurrent misses on one key (switchable). Its origin is a map that counts the requests it serves and takes
/// `origin_latency` per request, so concurrent misses overlap as they would at a real origin.
pub struct Memory {
    origin: Mutex<HashMap<String, String>>,
    origin_hits: AtomicU64,
    cache: Mutex<HashMap<String, (String, Instant)>>,
    flights: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    pub ttl: Duration,
    pub invalidate: bool,
    pub coalesce: bool,
    pub origin_latency: Duration,
}

impl Memory {
    /// A cache with a TTL of a minute, the given behaviour and an origin that takes 20 ms per request.
    pub fn new(invalidate: bool, coalesce: bool) -> Memory {
        Memory {
            origin: Mutex::new(HashMap::new()),
            origin_hits: AtomicU64::new(0),
            cache: Mutex::new(HashMap::new()),
            flights: Mutex::new(HashMap::new()),
            ttl: Duration::from_secs(60),
            invalidate,
            coalesce,
            origin_latency: Duration::from_millis(20),
        }
    }

    fn cached(&self, key: &str) -> Option<String> {
        let cache = self.cache.lock().ok()?;
        let (value, stored) = cache.get(key)?;
        (stored.elapsed() < self.ttl).then(|| value.clone())
    }

    fn fetch(&self, key: &str) -> String {
        thread::sleep(self.origin_latency);
        self.origin_hits.fetch_add(1, Ordering::SeqCst);
        let value = self
            .origin
            .lock()
            .ok()
            .and_then(|origin| origin.get(key).cloned())
            .unwrap_or_default();
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(key.to_string(), (value.clone(), Instant::now()));
        }
        value
    }
}

impl Subject for Memory {
    fn read(&self, key: &str) -> Result<(String, bool), String> {
        if let Some(value) = self.cached(key) {
            return Ok((value, true));
        }
        if !self.coalesce {
            return Ok((self.fetch(key), false));
        }
        // One miss fetches; the others wait on its flight, then find the value cached.
        let flight = {
            let mut flights = self
                .flights
                .lock()
                .map_err(|_| "flights poisoned".to_string())?;
            Arc::clone(flights.entry(key.to_string()).or_default())
        };
        let _turn = flight.lock().map_err(|_| "flight poisoned".to_string())?;
        match self.cached(key) {
            Some(value) => Ok((value, true)),
            None => Ok((self.fetch(key), false)),
        }
    }

    fn write(&self, key: &str, value: &str) -> Result<(), String> {
        self.origin
            .lock()
            .map_err(|_| "origin poisoned".to_string())?
            .insert(key.to_string(), value.to_string());
        if self.invalidate
            && let Ok(mut cache) = self.cache.lock()
        {
            cache.remove(key);
        }
        Ok(())
    }

    fn origin_requests(&self) -> Result<u64, String> {
        Ok(self.origin_hits.load(Ordering::SeqCst))
    }
}

/// The measurements of a cache run, recorded at the run under [`names`].
pub fn run(stage: &CacheStage, subject: &dyn Subject) -> Vec<Metric> {
    let mut out = Vec::new();
    let (requests, hits, errors) = profile(stage, subject);
    let hit_percent = if requests == 0 {
        0.0
    } else {
        hits as f64 * 100.0 / requests as f64
    };
    out.push(metric("cache.profile.requests", requests as f64));
    out.push(metric("cache.profile.hits", hits as f64));
    out.push(metric("cache.profile.errors", errors as f64));
    out.push(metric("cache.profile.hit_percent", hit_percent));
    out.push(metric(
        "cache.invalidation.ms",
        invalidation(stage, subject),
    ));
    let (origin, stampede_errors) = stampede(stage, subject);
    out.push(metric("cache.stampede.origin_requests", origin));
    out.push(metric("cache.stampede.errors", stampede_errors as f64));
    out
}

fn metric(name: &str, value: f64) -> Metric {
    Metric {
        name: name.to_string(),
        value,
    }
}

/// The profile's keys: a deterministic stream, 80 % of its reads over the hottest fifth of the keys.
pub fn profile_keys(keys: u32, requests: u32) -> Vec<String> {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let hot = (keys / 5).max(1) as u64;
    (0..requests)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let r = state >> 33;
            let idx = if r % 10 < 8 {
                (r / 10) % hot
            } else {
                (r / 10) % u64::from(keys)
            };
            format!("item-{idx}")
        })
        .collect()
}

/// The profile's requests, hits and errors, read in order.
fn profile(stage: &CacheStage, subject: &dyn Subject) -> (u64, u64, u64) {
    let mut hits = 0;
    let mut errors = 0;
    let keys = profile_keys(stage.keys, stage.requests);
    for key in &keys {
        match subject.read(key) {
            Ok((_, true)) => hits += 1,
            Ok((_, false)) => {}
            Err(_) => errors += 1,
        }
    }
    (keys.len() as u64, hits, errors)
}

/// The milliseconds from a write until a read returns the written value, or [`UNREADABLE`] when it does not within the
/// bound (a cache that keeps the old value serves it until its TTL).
fn invalidation(stage: &CacheStage, subject: &dyn Subject) -> f64 {
    let key = "apz-invalidation";
    if subject.write(key, "v1").is_err() || subject.read(key).is_err() {
        return UNREADABLE;
    }
    if subject.write(key, "v2").is_err() {
        return UNREADABLE;
    }
    let started = Instant::now();
    let bound = Duration::from_millis(stage.invalidation_within_ms);
    loop {
        match subject.read(key) {
            Ok((value, _)) if value == "v2" => return started.elapsed().as_secs_f64() * 1000.0,
            _ => {}
        }
        if started.elapsed() >= bound {
            return UNREADABLE;
        }
        thread::sleep(Duration::from_millis(2));
    }
}

/// The origin requests a wave of concurrent misses on one key caused, and the reads that failed.
fn stampede(stage: &CacheStage, subject: &dyn Subject) -> (f64, u64) {
    let key = "apz-stampede";
    if subject.write(key, "h1").is_err() {
        return (UNREADABLE, u64::from(stage.stampede_clients));
    }
    let Ok(before) = subject.origin_requests() else {
        return (UNREADABLE, 0);
    };
    let clients = stage.stampede_clients as usize;
    let barrier = Barrier::new(clients);
    let errors: u64 = thread::scope(|scope| {
        let handles: Vec<_> = (0..clients)
            .map(|_| {
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    u64::from(subject.read(key).is_err())
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or(1)).sum()
    });
    match subject.origin_requests() {
        Ok(after) => ((after.saturating_sub(before)) as f64, errors),
        Err(_) => (UNREADABLE, errors),
    }
}

/// The judgments of a cache stage over the values the run recorded: each item as `(id, passed, detail, measured)`.
pub fn judge(stage: &CacheStage, values: &[Metric]) -> Vec<(String, bool, String, bool)> {
    let mut out = Vec::new();
    match (
        value(values, "cache.profile.hit_percent"),
        value(values, "cache.profile.errors"),
        value(values, "cache.profile.requests"),
    ) {
        (Some(percent), Some(errors), Some(requests)) => {
            let passed = percent >= f64::from(stage.hit_floor_percent) && errors == 0.0;
            out.push((
                "profile".into(),
                passed,
                format!(
                    "the hit rate over {requests:.0} requests is {percent:.1} % ({errors:.0} failed; the floor is {} %)",
                    stage.hit_floor_percent
                ),
                true,
            ));
        }
        _ => out.push(not_run("profile", "cache profile")),
    }
    match value(values, "cache.invalidation.ms") {
        Some(ms) => {
            let within = stage.invalidation_within_ms as f64;
            let passed = ms != UNREADABLE && (0.0..=within).contains(&ms);
            let detail = if ms == UNREADABLE {
                format!(
                    "a write was not read back within {} ms: the old value is served",
                    stage.invalidation_within_ms
                )
            } else {
                format!(
                    "a write is read back in {ms:.1} ms (the bound is {} ms)",
                    stage.invalidation_within_ms
                )
            };
            out.push(("invalidation".into(), passed, detail, true));
        }
        None => out.push(not_run("invalidation", "cache invalidation")),
    }
    match (
        value(values, "cache.stampede.origin_requests"),
        value(values, "cache.stampede.errors"),
    ) {
        (Some(origin), Some(errors)) => {
            let passed =
                origin != UNREADABLE && origin <= stage.stampede_origin_max as f64 && errors == 0.0;
            let shown = if origin == UNREADABLE {
                "unknown".into()
            } else {
                format!("{origin:.0}")
            };
            out.push((
                "stampede".into(),
                passed,
                format!(
                    "{} concurrent misses on one key reached the origin {shown} times (at most {}; {errors:.0} failed)",
                    stage.stampede_clients, stage.stampede_origin_max
                ),
                true,
            ));
        }
        _ => out.push(not_run("stampede", "cache stampede")),
    }
    out
}
