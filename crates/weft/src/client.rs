//! A client of a Weft server: the hub's `weft serve`, a node's cache, or apid's door to either.
//!
//! A base URL names the server and the path its API sits under: `http://127.0.0.1:7461/v1` for `weft serve`,
//! `https://hub:8443/v1/weft` for the owner through apid, `…/internal/v1/agents/107/gateway/weft` for an agent VM.

use std::time::Duration;

use apz_http::client::{Reply, Target, request};
use serde_json::{Value, json};

use crate::hash::Hash;
use crate::wire::{self, BUNDLE_BYTES, Upload};

#[derive(Clone, Debug)]
pub struct Client {
    pub target: Target,
    pub prefix: String,
    pub timeout: Duration,
}

impl Client {
    /// `base` is `http(s)://host:port/prefix`. `headers` go on every request (the internal token, the caller).
    pub fn new(
        base: &str,
        bearer: Option<String>,
        headers: Vec<(String, String)>,
    ) -> Result<Client, String> {
        let (scheme, rest) = base
            .split_once("://")
            .ok_or_else(|| format!("{base}: not a URL"))?;
        let (host, prefix) = match rest.find('/') {
            Some(i) => (&rest[..i], rest[i..].trim_end_matches('/').to_string()),
            None => (rest, String::new()),
        };
        Ok(Client {
            target: Target {
                base: format!("{scheme}://{host}"),
                pin: None,
                bearer,
                headers,
            },
            prefix,
            timeout: Duration::from_secs(60),
        })
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Client {
        self.target
            .headers
            .retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        self.target
            .headers
            .push((name.to_string(), value.to_string()));
        self
    }

    fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
        extra: &[(&str, &str)],
    ) -> Result<Reply, String> {
        let mut target = self.target.clone();
        for (n, v) in extra {
            target.headers.push((n.to_string(), v.to_string()));
        }
        let url = format!("{}{path}", self.prefix);
        let reply = request(&target, method, &url, body, self.timeout)
            .map_err(|e| format!("{} {url}: {e}", self.target.base))?;
        if reply.status >= 400 {
            let msg = serde_json::from_slice::<Value>(&reply.body)
                .ok()
                .and_then(|v| v["error"].as_str().map(str::to_string))
                .unwrap_or_else(|| String::from_utf8_lossy(&reply.body).trim().to_string());
            return Err(format!(
                "{} ({})",
                if msg.is_empty() {
                    "refused".into()
                } else {
                    msg
                },
                reply.status
            ));
        }
        Ok(reply)
    }

    pub fn get(&self, path: &str) -> Result<Value, String> {
        let r = self.call("GET", path, None, &[])?;
        serde_json::from_slice(&r.body).map_err(|e| format!("GET {path}: {e}"))
    }

    pub fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
        let r = self.call(
            "POST",
            path,
            Some(body.to_string().as_bytes()),
            &[("Content-Type", "application/json")],
        )?;
        if r.body.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&r.body).map_err(|e| format!("POST {path}: {e}"))
    }

    pub fn get_bytes(&self, path: &str) -> Result<Vec<u8>, String> {
        Ok(self.call("GET", path, None, &[])?.body)
    }

    pub fn get_range(&self, path: &str, start: usize, end: usize) -> Result<Vec<u8>, String> {
        let range = format!("bytes={start}-{}", end - 1);
        Ok(self.call("GET", path, None, &[("Range", &range)])?.body)
    }

    pub fn put_bytes(&self, path: &str, bytes: &[u8]) -> Result<Value, String> {
        let r = self.call(
            "PUT",
            path,
            Some(bytes),
            &[("Content-Type", "application/octet-stream")],
        )?;
        Ok(serde_json::from_slice(&r.body).unwrap_or(Value::Null))
    }

    pub fn post_for_bytes(&self, path: &str, body: &Value) -> Result<Vec<u8>, String> {
        Ok(self
            .call(
                "POST",
                path,
                Some(body.to_string().as_bytes()),
                &[("Content-Type", "application/json")],
            )?
            .body)
    }

    /// Which of `hashes` the server lacks.
    pub fn missing(&self, hashes: &[Hash]) -> Result<Vec<Hash>, String> {
        let mut out = Vec::new();
        for part in hashes.chunks(4096) {
            let v = self.post("/wire/missing", &json!({ "hashes": part }))?;
            out.extend(
                serde_json::from_value::<Vec<Hash>>(v["missing"].clone())
                    .map_err(|e| e.to_string())?,
            );
        }
        Ok(out)
    }

    /// Uploads whichever of `hashes` the server lacks; `read` gives an object's bytes.
    pub fn upload(
        &self,
        hashes: &[Hash],
        read: &dyn Fn(&Hash) -> Result<Vec<u8>, String>,
    ) -> Result<usize, String> {
        let missing = self.missing(hashes)?;
        for h in &missing {
            let bytes = read(h)?;
            match wire::upload_plan(&bytes) {
                Upload::Whole(hash) => {
                    self.put_bytes(&format!("/wire/objects/{}", hash.hex()), &bytes)?;
                }
                Upload::Chunked { hash, chunks } => {
                    let ids: Vec<Hash> = chunks.iter().map(|c| c.0).collect();
                    let lacking = self.missing(&ids)?;
                    for (ch, off, len) in &chunks {
                        if lacking.contains(ch) {
                            self.put_bytes(
                                &format!("/wire/objects/{}", ch.hex()),
                                &bytes[*off..off + len],
                            )?;
                        }
                    }
                    let list: Vec<(Hash, u64)> =
                        chunks.iter().map(|(h, _, l)| (*h, *l as u64)).collect();
                    self.post("/wire/manifest", &json!({ "hash": hash, "chunks": list }))?;
                }
            }
        }
        Ok(missing.len())
    }

    /// Fetches `hashes` (the caller has already dropped those it holds), handing each object to `put`.
    pub fn download(
        &self,
        hashes: &[Hash],
        put: &mut dyn FnMut(&Hash, &[u8]) -> Result<(), String>,
    ) -> Result<usize, String> {
        let mut whole = Vec::new();
        let mut count = 0;
        let mut infos = Vec::new();
        for part in hashes.chunks(2048) {
            let v = self.post("/wire/info", &json!({ "hashes": part }))?;
            infos.extend(v["objects"].as_array().cloned().unwrap_or_default());
        }
        for info in infos {
            let h: Hash =
                serde_json::from_value(info["hash"].clone()).map_err(|e| e.to_string())?;
            let h = &h;
            let size = info["size"].as_u64().unwrap_or(0) as usize;
            match info["chunks"].as_array() {
                Some(chunks) => {
                    // A large object: its chunks in bundles, then put together here.
                    let list: Vec<(Hash, u64)> =
                        serde_json::from_value(Value::Array(chunks.clone()))
                            .map_err(|e| e.to_string())?;
                    let mut parts: std::collections::HashMap<Hash, Vec<u8>> =
                        std::collections::HashMap::new();
                    let ids: Vec<Hash> = list.iter().map(|c| c.0).collect();
                    self.bundles(&ids, &mut |ch, b| {
                        parts.insert(*ch, b.to_vec());
                        Ok(())
                    })?;
                    let mut bytes = Vec::with_capacity(size);
                    for (ch, _) in &list {
                        bytes.extend_from_slice(
                            parts
                                .get(ch)
                                .ok_or_else(|| format!("chunk {} did not come", ch.short()))?,
                        );
                    }
                    if Hash::of(&bytes) != *h {
                        return Err(format!("object {} does not match its hash", h.short()));
                    }
                    put(h, &bytes)?;
                    count += 1;
                }
                None => whole.push((*h, size)),
            }
        }
        // Objects stored whole, packed into bundles.
        let mut batch = Vec::new();
        let mut bytes = 0;
        for (h, size) in whole {
            if bytes + size > BUNDLE_BYTES && !batch.is_empty() {
                count += self.bundles(&batch, put)?;
                batch.clear();
                bytes = 0;
            }
            batch.push(h);
            bytes += size;
        }
        if !batch.is_empty() {
            count += self.bundles(&batch, put)?;
        }
        Ok(count)
    }

    fn bundles(
        &self,
        hashes: &[Hash],
        put: &mut dyn FnMut(&Hash, &[u8]) -> Result<(), String>,
    ) -> Result<usize, String> {
        let mut n = 0;
        let mut rest: Vec<Hash> = hashes.to_vec();
        while !rest.is_empty() {
            let bytes = self.post_for_bytes("/wire/bundle", &json!({ "hashes": rest }))?;
            let objects = wire::decode_bundle(&bytes)?;
            if objects.is_empty() {
                return Err("the server sent an empty bundle".into());
            }
            for (h, b) in &objects {
                put(h, b)?;
                n += 1;
            }
            let got: std::collections::HashSet<Hash> = objects.iter().map(|o| o.0).collect();
            rest.retain(|h| !got.contains(h));
        }
        Ok(n)
    }
}
