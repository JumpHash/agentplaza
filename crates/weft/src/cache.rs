//! `weft serve --cache`: the object cache every node runs for its VMs.
//!
//! Stateless apart from its objects: it answers object reads from disk, fetches what it lacks from upstream (the hub
//! through apid, or another cache), and forwards everything else unchanged, uploads included (write-through). Objects
//! are immutable, so nothing ever needs invalidating; the least recently used go when the cache is over its size.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use apz_http::auth::InternalToken;
use apz_http::client::request;
use apz_http::{Handle, Limits, Request, Response, Server};
use serde_json::{Value, json};

use crate::client::Client;
use crate::hash::Hash;
use crate::wire;

pub struct Cache {
    dir: PathBuf,
    upstream: Client,
    max_bytes: u64,
    bytes: AtomicU64,
    /// Last use of each object, by a counter (not a clock), for eviction.
    used: Mutex<HashMap<Hash, u64>>,
    tick: AtomicU64,
    pub hits: AtomicU64,
    pub misses: AtomicU64,
}

impl Cache {
    pub fn open(dir: &Path, upstream: Client, max_bytes: u64) -> Result<Arc<Cache>, String> {
        let objects = dir.join("objects");
        std::fs::create_dir_all(&objects).map_err(|e| format!("{}: {e}", objects.display()))?;
        let mut used = HashMap::new();
        let mut bytes = 0;
        for shard in std::fs::read_dir(&objects)
            .map_err(|e| e.to_string())?
            .flatten()
        {
            for f in std::fs::read_dir(shard.path())
                .into_iter()
                .flatten()
                .flatten()
            {
                if let Ok(h) = format!(
                    "{}{}",
                    shard.file_name().to_string_lossy(),
                    f.file_name().to_string_lossy()
                )
                .parse::<Hash>()
                {
                    bytes += f.metadata().map(|m| m.len()).unwrap_or(0);
                    used.insert(h, 0);
                }
            }
        }
        Ok(Arc::new(Cache {
            dir: dir.to_path_buf(),
            upstream,
            max_bytes,
            bytes: AtomicU64::new(bytes),
            used: Mutex::new(used),
            tick: AtomicU64::new(1),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }))
    }

    fn path(&self, h: &Hash) -> PathBuf {
        let hex = h.hex();
        self.dir.join("objects").join(&hex[..2]).join(&hex[2..])
    }

    pub fn get_local(&self, h: &Hash) -> Option<Vec<u8>> {
        let b = std::fs::read(self.path(h)).ok()?;
        if Hash::of(&b) != *h {
            let _ = std::fs::remove_file(self.path(h));
            return None;
        }
        self.used
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(*h, self.tick.fetch_add(1, Ordering::Relaxed));
        Some(b)
    }

    pub fn put_local(&self, h: &Hash, bytes: &[u8]) {
        if Hash::of(bytes) != *h || self.path(h).exists() {
            return;
        }
        let p = self.path(h);
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        let tmp = p.with_extension("tmp");
        if std::fs::write(&tmp, bytes)
            .and_then(|_| std::fs::rename(&tmp, &p))
            .is_ok()
        {
            self.bytes.fetch_add(bytes.len() as u64, Ordering::Relaxed);
            self.used
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(*h, self.tick.fetch_add(1, Ordering::Relaxed));
            self.evict();
        }
    }

    fn evict(&self) {
        if self.bytes.load(Ordering::Relaxed) <= self.max_bytes {
            return;
        }
        let mut used = self.used.lock().unwrap_or_else(|p| p.into_inner());
        let mut order: Vec<(u64, Hash)> = used.iter().map(|(h, t)| (*t, *h)).collect();
        order.sort();
        for (_, h) in order {
            if self.bytes.load(Ordering::Relaxed) <= self.max_bytes * 9 / 10 {
                break;
            }
            let p = self.path(&h);
            let len = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            if std::fs::remove_file(&p).is_ok() {
                self.bytes.fetch_sub(len, Ordering::Relaxed);
            }
            used.remove(&h);
        }
    }

    /// The upstream client for one request: the caller it names goes along.
    fn up(&self, req: &Request) -> Client {
        match req.header(wire::CALLER) {
            Some(c) => self.upstream.clone().with_header(wire::CALLER, c),
            None => self.upstream.clone(),
        }
    }

    fn handle(&self, req: &Request) -> Response {
        let path = req.path.as_str();
        if req.method == "GET" && path == "/v1/cache" {
            return Response::json(
                200,
                &json!({ "objects": self.used.lock().map(|u| u.len()).unwrap_or(0), "bytes": self.bytes.load(Ordering::Relaxed), "max_bytes": self.max_bytes, "hits": self.hits.load(Ordering::Relaxed), "misses": self.misses.load(Ordering::Relaxed) }),
            );
        }
        if req.method == "GET"
            && let Some(hex) = path.strip_prefix("/v1/wire/objects/")
            && let Ok(h) = hex.parse::<Hash>()
        {
            let bytes = match self.get_local(&h) {
                Some(b) => {
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    b
                }
                None => {
                    self.misses.fetch_add(1, Ordering::Relaxed);
                    match self.up(req).get_bytes(&format!("/wire/objects/{hex}")) {
                        Ok(b) if Hash::of(&b) == h => {
                            self.put_local(&h, &b);
                            b
                        }
                        Ok(_) => return Response::error(502, "upstream sent the wrong bytes"),
                        Err(e) => return Response::error(502, &e),
                    }
                }
            };
            return match req
                .header("range")
                .and_then(|r| wire::parse_range(r, bytes.len()))
            {
                Some((a, z)) => {
                    Response::new(206, "application/octet-stream", bytes[a..z].to_vec())
                        .with_header(
                            "Content-Range",
                            &format!("bytes {a}-{}/{}", z - 1, bytes.len()),
                        )
                }
                None => Response::new(200, "application/octet-stream", bytes),
            };
        }
        if req.method == "POST" && path == "/v1/wire/bundle" {
            let hashes: Vec<Hash> = match req
                .json::<Value>()
                .ok()
                .and_then(|v| serde_json::from_value(v["hashes"].clone()).ok())
            {
                Some(h) => h,
                None => return Response::error(400, "bundle needs hashes"),
            };
            let mut have = Vec::new();
            let mut lack = Vec::new();
            for h in hashes {
                match self.get_local(&h) {
                    Some(b) => have.push((h, b)),
                    None => lack.push(h),
                }
            }
            self.hits.fetch_add(have.len() as u64, Ordering::Relaxed);
            self.misses.fetch_add(lack.len() as u64, Ordering::Relaxed);
            if !lack.is_empty() {
                let mut got = Vec::new();
                let r = self.up(req).download(&lack, &mut |h, b| {
                    got.push((*h, b.to_vec()));
                    Ok(())
                });
                if let Err(e) = r {
                    return Response::error(502, &e);
                }
                for (h, b) in got {
                    self.put_local(&h, &b);
                    have.push((h, b));
                }
            }
            // One bundle at most; the client asks again for the rest.
            let mut size = 0;
            let mut out = Vec::new();
            for (h, b) in have {
                if !out.is_empty() && size + b.len() > wire::BUNDLE_BYTES {
                    break;
                }
                size += b.len();
                out.push((h, b));
            }
            return Response::new(200, "application/octet-stream", wire::encode_bundle(&out));
        }
        if req.method == "PUT"
            && let Some(hex) = path.strip_prefix("/v1/wire/objects/")
            && let Ok(h) = hex.parse::<Hash>()
        {
            if Hash::of(&req.body) != h {
                return Response::error(400, "the bytes do not match the hash");
            }
            self.put_local(&h, &req.body);
        }
        // Everything else goes upstream as it came.
        self.forward(req)
    }

    fn forward(&self, req: &Request) -> Response {
        let up = self.up(req);
        let mut target = up.target.clone();
        for name in ["range", "content-type"] {
            if let Some(v) = req.header(name) {
                target.headers.push((name.to_string(), v.to_string()));
            }
        }
        let rest = req.path.strip_prefix("/v1").unwrap_or(&req.path);
        let url = if req.query.is_empty() {
            format!("{}{rest}", up.prefix)
        } else {
            format!("{}{rest}?{}", up.prefix, req.query)
        };
        let body = (!req.body.is_empty() || req.method == "POST" || req.method == "PUT")
            .then_some(&req.body[..]);
        match request(&target, &req.method, &url, body, Duration::from_secs(120)) {
            Ok(r) => Response::new(r.status, &r.content_type, r.body),
            Err(e) => Response::error(502, &format!("upstream: {e}")),
        }
    }
}

/// Serves a cache on `127.0.0.1:port` behind `token`.
pub fn spawn(cache: Arc<Cache>, port: u16, token: InternalToken) -> io::Result<Handle> {
    let handler = move |req: &Request| cache.handle(req);
    Server::bind(port)?
        .limits(Limits {
            max_head: 32 << 10,
            max_body: 24 << 20,
        })
        .spawn(token.guard(handler))
}
