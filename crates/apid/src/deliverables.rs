//! The Library: deliverables agents hand in (files they were asked to produce), kept in the
//! plaza's object store — Garage over S3 when configured, a local folder otherwise.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use apz_gateway::s3::S3;
use apz_http::{Params, Request, Response, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::internal::authorized;
use crate::providers::write_atomic;
use crate::state::App;
use crate::vault::OnBehalf;

/// The longest name a deliverable may carry.
const NAME_CAP: usize = 120;
/// The index file, under `<state-dir>/files/`.
const INDEX: &str = "index.json";

/// One file an agent handed in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deliverable {
    pub id: String,
    pub name: String,
    pub title: String,
    pub description: Option<String>,
    pub content_type: String,
    pub size: u64,
    pub author_cid: u64,
    pub author: String,
    pub task: Option<String>,
    pub created_ms: u64,
    pub sha256: String,
    /// "garage" or "local".
    pub store: String,
    /// "deliverable" (handed in with deliver_file) or "shared" (share_file).
    #[serde(default = "deliverable_kind")]
    pub kind: String,
    /// "public" (every agent, the board) or "private" (author, `allowed`, the owner).
    #[serde(default = "public_audience")]
    pub audience: String,
    /// Agents a private file is shared with.
    #[serde(default)]
    pub allowed: Vec<u64>,
}

fn deliverable_kind() -> String {
    "deliverable".to_string()
}

fn public_audience() -> String {
    "public".to_string()
}

/// What an upload says about itself; the bytes travel apart from it.
pub struct Upload {
    pub name: String,
    pub title: String,
    pub description: Option<String>,
    pub task: Option<String>,
    pub content_type: String,
    pub author_cid: u64,
    pub author: String,
    pub kind: String,
    pub audience: String,
    pub allowed: Vec<u64>,
}

pub struct Store {
    /// `<state-dir>/files`: the index, and the blobs when they are kept locally.
    dir: PathBuf,
    /// Garage's address (endpoint, bucket, region), when configured. Its key pair is the vault's (`tool:garage`).
    garage: Option<Garage>,
    /// The vault the Garage keys are read from, per use (it may be sealed at load).
    vault: Arc<crate::vault::Store>,
    index: Mutex<Vec<Deliverable>>,
}

impl Store {
    /// The file's record, if it exists.
    pub fn meta(&self, id: &str) -> Option<Deliverable> {
        self.find(id)
    }

    /// Whether agent `cid` may read file `id` (its author, a public file, or one shared with it).
    pub fn can_read(&self, id: &str, cid: u64) -> bool {
        self.find(id)
            .is_some_and(|d| d.audience == "public" || d.author_cid == cid || d.allowed.contains(&cid))
    }

    /// Makes file `id` public (attached to a board post or comment). Only its author may.
    pub fn publish(&self, id: &str, by: u64) -> Result<(), String> {
        let mut index = lock(&self.index);
        let pos = index.iter().position(|d| d.id == id).ok_or_else(|| "no such file".to_string())?;
        if index[pos].audience == "public" {
            return Ok(());
        }
        if by != index[pos].author_cid {
            return Err("only the file's author can make it public".to_string());
        }
        let mut next = index.to_vec();
        next[pos].audience = "public".to_string();
        self.save(&next).map_err(|e| format!("cannot save the index: {e}"))?;
        *index = next;
        Ok(())
    }

    /// Gives agents `to` access to file `id` (attached to a direct message). Only its author may.
    pub fn grant(&self, id: &str, by: u64, to: &[u64]) -> Result<(), String> {
        let mut index = lock(&self.index);
        let pos = index.iter().position(|d| d.id == id).ok_or_else(|| "no such file".to_string())?;
        if index[pos].audience == "public" {
            return Ok(());
        }
        if by != index[pos].author_cid {
            return Err("only the file's author can share it".to_string());
        }
        let mut next = index.to_vec();
        for cid in to {
            if !next[pos].allowed.contains(cid) {
                next[pos].allowed.push(*cid);
            }
        }
        self.save(&next).map_err(|e| format!("cannot save the index: {e}"))?;
        *index = next;
        Ok(())
    }

    /// Loads `<state-dir>/files/index.json` (a missing file is an empty library) and picks
    /// the store: Garage when the environment configures it, else a local folder.
    pub fn load(state_dir: &Path, env: &BTreeMap<String, String>, vault: Arc<crate::vault::Store>) -> io::Result<Self> {
        let dir = state_dir.join("files");
        std::fs::create_dir_all(&dir)?;
        let index: Vec<Deliverable> = match std::fs::read(dir.join(INDEX)) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        for name in GARAGE_KEYS {
            vault.register_consumer(GARAGE_BINDING, "Garage object store", name);
        }
        Ok(Self { dir, garage: garage_from(env), vault, index: Mutex::new(index) })
    }

    /// Stores the bytes and indexes them. The bytes go first, so an index entry never names
    /// a missing object; a failed index save removes them again.
    pub fn add(&self, upload: Upload, body: &[u8]) -> Result<Deliverable, String> {
        let store = if self.garage.is_some() && self.keys().is_some() { "garage" } else { "local" };
        let item = Deliverable {
            id: new_id(),
            name: upload.name,
            title: upload.title,
            description: upload.description,
            content_type: upload.content_type,
            size: body.len() as u64,
            author_cid: upload.author_cid,
            author: upload.author,
            task: upload.task,
            created_ms: now_ms(),
            sha256: sha256_hex(body),
            store: store.to_string(),
            kind: upload.kind,
            audience: upload.audience,
            allowed: upload.allowed,
        };
        self.write_blob(&item, body)?;
        let mut index = lock(&self.index);
        let mut next = index.to_vec();
        next.push(item.clone());
        if let Err(e) = self.save(&next) {
            let _ = self.delete_blob(&item);
            return Err(format!("cannot save the index: {e}"));
        }
        *index = next;
        Ok(item)
    }

    /// Every deliverable, newest first.
    pub fn list(&self) -> Vec<Deliverable> {
        lock(&self.index).iter().rev().cloned().collect()
    }

    pub fn find(&self, id: &str) -> Option<Deliverable> {
        lock(&self.index).iter().find(|d| d.id == id).cloned()
    }

    /// The bytes of one deliverable, from wherever it is kept.
    pub fn read_blob(&self, item: &Deliverable) -> Result<Vec<u8>, String> {
        if item.store == "garage" {
            return self.garage()?.get(&object_key(item));
        }
        std::fs::read(self.blob_path(&item.id)).map_err(|e| e.to_string())
    }

    /// Removes a deliverable's bytes, then its entry. Returns the entry, or None when unknown.
    pub fn remove(&self, id: &str) -> Result<Option<Deliverable>, String> {
        let Some(item) = self.find(id) else { return Ok(None) };
        self.delete_blob(&item)?;
        let mut index = lock(&self.index);
        let next: Vec<Deliverable> = index.iter().filter(|d| d.id != id).cloned().collect();
        self.save(&next).map_err(|e| format!("cannot save the index: {e}"))?;
        *index = next;
        Ok(Some(item))
    }

    /// Garage's key pair from the vault (`tool:garage`), or the process environment when no entry of that name is
    /// bound to it. Read per use.
    fn keys(&self) -> Option<(String, String)> {
        let access = self.vault.value_for(GARAGE_KEYS[0], GARAGE_BINDING, &OnBehalf::Hub)?;
        let secret = self.vault.value_for(GARAGE_KEYS[1], GARAGE_BINDING, &OnBehalf::Hub)?;
        Some((access, secret))
    }

    /// A client for Garage: the address from the settings, the keys from the vault.
    fn garage(&self) -> Result<S3, String> {
        let address = self.garage.as_ref().ok_or_else(|| "the file is in Garage, which is not configured".to_string())?;
        let (access_key, secret_key) = self.keys().ok_or_else(|| "the Garage keys are not in the vault (or the vault is sealed)".to_string())?;
        Ok(S3 {
            endpoint: address.endpoint.clone(),
            bucket: address.bucket.clone(),
            region: address.region.clone(),
            access_key,
            secret_key,
        })
    }

    fn save(&self, index: &[Deliverable]) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(index)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        write_atomic(&self.dir.join(INDEX), &bytes, 0o600)
    }

    /// Whether Garage is configured; then the objects below are kept there.
    pub fn has_garage(&self) -> bool {
        self.garage.is_some()
    }

    /// Writes an object that is not a deliverable (skill files): Garage when it is configured, else
    /// `<dir>/objects/<key>`. Not indexed.
    pub fn put_object(&self, key: &str, bytes: &[u8]) -> Result<(), String> {
        if self.garage.is_some() {
            return self.garage()?.put(key, bytes, "application/octet-stream");
        }
        let path = self.object_path(key)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        write_atomic(&path, bytes, 0o600).map_err(|e| e.to_string())
    }

    /// Reads an object `put_object` wrote.
    pub fn get_object(&self, key: &str) -> Result<Vec<u8>, String> {
        if self.garage.is_some() {
            return self.garage()?.get(key);
        }
        std::fs::read(self.object_path(key)?).map_err(|e| e.to_string())
    }

    /// Deletes an object `put_object` wrote. A missing one is not an error.
    pub fn delete_object(&self, key: &str) -> Result<(), String> {
        if self.garage.is_some() {
            return self.garage()?.delete(key);
        }
        match std::fs::remove_file(self.object_path(key)?) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// `<dir>/objects/<key>`, for a key of plain segments only.
    fn object_path(&self, key: &str) -> Result<PathBuf, String> {
        if key.is_empty() || key.split('/').any(|s| s.is_empty() || s == "." || s == "..") {
            return Err(format!("{key:?} is not an object key"));
        }
        Ok(self.dir.join("objects").join(key))
    }

    fn blob_path(&self, id: &str) -> PathBuf {
        self.dir.join("blobs").join(id)
    }

    fn write_blob(&self, item: &Deliverable, body: &[u8]) -> Result<(), String> {
        if item.store == "garage" {
            return self.garage()?.put(&object_key(item), body, &item.content_type);
        }
        std::fs::create_dir_all(self.dir.join("blobs")).map_err(|e| e.to_string())?;
        write_atomic(&self.blob_path(&item.id), body, 0o600).map_err(|e| e.to_string())
    }

    fn delete_blob(&self, item: &Deliverable) -> Result<(), String> {
        if item.store == "garage" {
            return self.garage()?.delete(&object_key(item));
        }
        match std::fs::remove_file(self.blob_path(&item.id)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// `/v1/files…` (the console) and `/internal/v1/agents/{cid}/gateway/files…` (agents, via
/// the hub's plaza.gateway).
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/files", list_route)
        .get("/v1/files/{id}", meta_route)
        .get("/v1/files/{id}/content", content_route)
        .delete("/v1/files/{id}", delete_route)
        .route("PUT", "/internal/v1/agents/{cid}/gateway/files", put_route)
        .get("/internal/v1/agents/{cid}/gateway/files/{id}", agent_get_route)
}

/// Every deliverable, or only one `kind` when the query names it.
fn list_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let kind = query(req, "kind");
    let items: Vec<Deliverable> = app
        .deliverables
        .list()
        .into_iter()
        .filter(|d| kind.as_deref().map_or(true, |k| d.kind == k))
        .collect();
    Response::json(200, &items)
}

fn meta_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    match app.deliverables.meta(p.get("id")) {
        Some(item) => Response::json(200, &item),
        None => Response::error(404, "no such file"),
    }
}

fn content_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    match app.deliverables.find(p.get("id")) {
        Some(item) => serve(app, &item, req.query_param("download") == Some("1")),
        None => Response::error(404, "no such file"),
    }
}

fn delete_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    match app.deliverables.remove(p.get("id")) {
        Ok(Some(item)) => {
            app.bus.publish("file", &json!({ "id": item.id, "deleted": true }));
            Response::json(200, &json!({ "deleted": item.id }))
        }
        Ok(None) => Response::error(404, "no such file"),
        Err(e) => Response::error(500, &e),
    }
}

fn put_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    authorized(app, req, p.get("cid"), |cid| Ok(put_file(app, req, cid)))
}

fn agent_get_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    authorized(app, req, p.get("cid"), |cid| {
        Ok(match app.deliverables.find(p.get("id")) {
            Some(item) if app.deliverables.can_read(&item.id, cid) => serve(app, &item, false),
            _ => Response::error(404, "no such file"),
        })
    })
}

/// An agent's upload: the file is the body; `name`, `title`, `description` and `task` are the
/// query. The content type is the request's, else guessed from the name.
fn put_file(app: &App, req: &Request, cid: u64) -> Response {
    let Some(raw) = req.query_param("name") else {
        return Response::error(400, "name is required");
    };
    let name = match clean_name(&decode(raw)) {
        Ok(name) => name,
        Err(e) => return Response::error(400, &e),
    };
    if req.body.is_empty() {
        return Response::error(400, "the body is empty");
    }
    // Publishing a file is an artifact.publish, checked whichever way the agent reaches this route:
    // the share/deliver tools, or a PUT from its own VM (C-003b reads the content for impersonation).
    let role = app.agents.get(cid).map(|agent| agent.role).unwrap_or_default();
    let content = serde_json::Value::String(String::from_utf8_lossy(&req.body).into_owned());
    let artifact = jhrl::authz::Entity { ty: "Artifact".into(), id: name.clone() };
    if let Some(why) = app.laws.refuse_op(cid, &role, "artifact.publish", artifact, BTreeMap::new(), &content) {
        return Response::error(403, &why);
    }
    let kind = if query(req, "kind").as_deref() == Some("shared") { "shared" } else { "deliverable" };
    let to = match parse_cids(query(req, "to").as_deref()) {
        Ok(to) => to,
        Err(e) => return Response::error(400, &e),
    };
    let (audience, allowed) = if to.is_empty() { ("public", Vec::new()) } else { ("private", to) };
    let upload = Upload {
        kind: kind.to_string(),
        audience: audience.to_string(),
        allowed,
        title: query(req, "title").unwrap_or_else(|| name.clone()),
        description: query(req, "description"),
        task: query(req, "task"),
        content_type: optional(req.header("content-type")).unwrap_or_else(|| guess_type(&name).to_string()),
        author: app.agents.get(cid).map(|a| a.name).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("agent {cid}")),
        author_cid: cid,
        name,
    };
    match app.deliverables.add(upload, &req.body) {
        Ok(item) => {
            app.bus.publish("file", &item);
            Response::json(
                201,
                &json!({
                    "id": item.id,
                    "url": format!("http://plaza.gateway/files/{}", item.id),
                    "size": item.size,
                    "kind": item.kind,
                    "audience": item.audience,
                }),
            )
        }
        Err(e) => Response::error(500, &e),
    }
}

/// The content security policy of a served HTML file. `sandbox allow-scripts` gives the page an opaque origin (no
/// same-origin, no forms, no top navigation), so its script cannot reach the console's storage or the hub as the owner.
/// Its styles (inline `<style>` included) and images (data:, blob: and https:) load, so a report renders as its author
/// made it; nothing in the policy reaches the console's origin.
const HTML_CSP: &str = "sandbox allow-scripts; default-src 'self' data: blob: https:; script-src 'unsafe-inline' 'self' https:; style-src 'self' 'unsafe-inline' https:; img-src 'self' data: blob: https:";

/// A file is HTML when its content type says so or its name ends `.html` or `.htm`.
fn is_html(content_type: &str, name: &str) -> bool {
    let ext = name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase());
    content_type.starts_with("text/html") || matches!(ext.as_deref(), Some("html" | "htm"))
}

/// What a stored file is served as: its content type and its extra headers. HTML is served inline as text/html under
/// `HTML_CSP`, so the console's frame and a direct open both run it sandboxed. SVG (which can carry script) always
/// downloads, and `download=1` downloads anything.
pub(crate) fn served(content_type: &str, name: &str, download: bool) -> (String, Vec<(&'static str, String)>) {
    let html = is_html(content_type, name);
    let served_type = if html { "text/html; charset=utf-8".to_string() } else { content_type.to_string() };
    let mut headers = vec![("x-content-type-options", "nosniff".to_string())];
    if html {
        headers.push(("content-security-policy", HTML_CSP.to_string()));
    }
    if download || content_type.contains("svg") {
        headers.push(("content-disposition", format!("attachment; filename=\"{name}\"")));
    }
    (served_type, headers)
}

/// The bytes with their content type and headers (see `served`).
fn serve(app: &App, item: &Deliverable, download: bool) -> Response {
    let bytes = match app.deliverables.read_blob(item) {
        Ok(bytes) => bytes,
        Err(e) => return Response::error(500, &e),
    };
    let (content_type, headers) = served(&item.content_type, &item.name, download);
    headers.into_iter().fold(Response::new(200, &content_type, bytes), |response, (name, value)| response.with_header(name, &value))
}

/// The name as stored: its basename, with anything outside `[A-Za-z0-9._-]` made `_`. A name
/// that is empty, only dots, or longer than 120 characters is refused.
pub fn clean_name(raw: &str) -> Result<String, String> {
    let base = raw.rsplit(|c: char| c == '/' || c == '\\').next().unwrap_or("");
    let name: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
        .collect();
    if name.chars().all(|c| c == '.') {
        return Err("the name is empty".into());
    }
    if name.len() > NAME_CAP {
        return Err(format!("the name is longer than {NAME_CAP} characters"));
    }
    Ok(name)
}

/// The content type a name's extension suggests.
pub fn guess_type(name: &str) -> &'static str {
    let Some((_, ext)) = name.rsplit_once('.') else { return "application/octet-stream" };
    match ext.to_ascii_lowercase().as_str() {
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "pdf" => "application/pdf",
        "txt" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "json" => "application/json",
        "csv" => "text/csv",
        "html" => "text/html; charset=utf-8",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// The garage object key of a deliverable.
fn object_key(item: &Deliverable) -> String {
    format!("deliverables/{}/{}", item.id, item.name)
}

/// Garage's keys are the vault's entries of these names, bound to `tool:garage`.
const GARAGE_KEYS: [&str; 2] = ["APZ_S3_ACCESS_KEY", "APZ_S3_SECRET_KEY"];
const GARAGE_BINDING: &str = "tool:garage";

/// Garage's address: the settings that are not secrets.
struct Garage {
    endpoint: String,
    bucket: String,
    region: String,
}

/// Garage's address, when `APZ_S3_ENDPOINT` and `APZ_S3_BUCKET` are set (in the env file or the process).
fn garage_from(env: &BTreeMap<String, String>) -> Option<Garage> {
    let get = |key: &str| {
        env.get(key)
            .cloned()
            .or_else(|| std::env::var(key).ok())
            .filter(|v| !v.is_empty())
    };
    Some(Garage {
        endpoint: get("APZ_S3_ENDPOINT")?,
        bucket: get("APZ_S3_BUCKET")?,
        region: get("APZ_S3_REGION").unwrap_or_else(|| "garage".to_string()),
    })
}

/// `f` and twelve hex digits from the OS random source (the clock if that is unreadable).
fn new_id() -> String {
    let mut bytes = [0u8; 6];
    let random = apz_os::random::fill(&mut bytes);
    if random.is_err() {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        bytes.copy_from_slice(&nanos.to_be_bytes()[10..]);
    }
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("f{hex}")
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// A query value, percent-decoded (`+` is a space), trimmed; `None` when absent or empty.
fn query(req: &Request, key: &str) -> Option<String> {
    optional(req.query_param(key).map(decode).as_deref())
}

/// Percent-decoding of a query value; malformed escapes are kept as they are.
fn decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            // Both hex digits must be there: positions i + 1 and i + 2.
            b'%' if i + 2 < bytes.len() => {
                match u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            other => out.push(other),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn optional(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|v| !v.is_empty()).map(str::to_string)
}

/// The agent cids a `to` query lists, comma-separated (`203,207`); absent is none.
fn parse_cids(raw: Option<&str>) -> Result<Vec<u64>, String> {
    let Some(raw) = raw else { return Ok(Vec::new()) };
    raw.split(',')
        .map(|s| s.trim().parse::<u64>().map_err(|_| "to must list agent cids".to_string()))
        .collect()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}


impl Store {
    /// The whole state as JSON, for Postgres (`crate::db`); "" when there is nothing to save.
    pub fn snapshot(&self) -> String {
        serde_json::to_string_pretty(&*lock(&self.index)).unwrap_or_default()
    }

    /// Replaces the state with a [`Store::snapshot`]; a refusal changes nothing.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let next: Vec<Deliverable> = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let mut index = lock(&self.index);
        self.save(&next).map_err(|e| e.to_string())?;
        *index = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A booted vault in `dir`, as the hub has it at start.
    fn vault_in(dir: &Path) -> Arc<crate::vault::Store> {
        let vault = crate::vault::Store::open(dir, &dir.join("vault.key"), None);
        vault.boot(apz_plaza::now_ms()).expect("boot");
        Arc::new(vault)
    }

    #[test]
    fn query_values_are_percent_decoded() {
        assert_eq!(decode("Hello%20bottle+ad"), "Hello bottle ad");
        assert_eq!(decode("100%25"), "100%");
        assert_eq!(decode("50%"), "50%");
        assert_eq!(decode("%E2%9C%93"), "\u{2713}");
    }

    #[test]
    fn snapshot_restores_into_a_fresh_store() {
        let env = BTreeMap::new();
        let a = Store::load(&scratch("snapshot"), &env, vault_in(&scratch("snapshot-vault"))).unwrap();
        a.add(upload("report.txt"), b"hello").unwrap();
        let b = Store::load(&scratch("snapshot-restored"), &env, vault_in(&scratch("snapshot-restored-vault"))).unwrap();
        b.restore(&a.snapshot()).unwrap();
        assert_eq!(b.snapshot(), a.snapshot());
        assert_eq!(b.list().len(), 1);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apid-deliverables-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn upload(name: &str) -> Upload {
        Upload {
            name: name.to_string(),
            title: "A title".to_string(),
            description: None,
            task: Some("task-1".to_string()),
            content_type: guess_type(name).to_string(),
            author_cid: 7,
            author: "7".to_string(),
            kind: "deliverable".to_string(),
            audience: "public".to_string(),
            allowed: Vec::new(),
        }
    }

    #[test]
    fn put_list_get_delete_round_trip() {
        let dir = scratch("roundtrip");
        let env = BTreeMap::new();
        let store = Store::load(&dir, &env, vault_in(&dir)).unwrap();
        let body = b"hello deliverable".to_vec();
        let item = store.add(upload("report.txt"), &body).unwrap();
        assert!(item.id.starts_with('f') && item.id.len() == 13);
        assert_eq!(item.store, "local");
        assert_eq!(item.size, body.len() as u64);
        assert_eq!(item.sha256, sha256_hex(&body));
        assert_eq!(store.list().len(), 1);
        let found = store.find(&item.id).unwrap();
        assert_eq!(store.read_blob(&found).unwrap(), body);
        let reloaded = Store::load(&dir, &env, vault_in(&dir)).unwrap();
        assert_eq!(reloaded.list()[0].id, item.id);
        assert!(reloaded.remove(&item.id).unwrap().is_some());
        assert!(reloaded.list().is_empty());
        assert!(reloaded.find(&item.id).is_none());
        assert!(!dir.join("files").join("blobs").join(&item.id).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_are_basenames_of_safe_characters() {
        assert_eq!(clean_name("../../etc/passwd").unwrap(), "passwd");
        assert_eq!(clean_name("C:\\out\\my report (final).pdf").unwrap(), "my_report__final_.pdf");
        assert!(clean_name("..").is_err());
        assert!(clean_name("").is_err());
        assert!(clean_name(&"a".repeat(121)).is_err());
        assert_eq!(clean_name(&"a".repeat(120)).unwrap().len(), 120);
    }

    #[test]
    fn html_is_served_inline_under_the_sandbox_policy() {
        let (kind, headers) = served("text/html; charset=utf-8", "page.html", false);
        assert_eq!(kind, "text/html; charset=utf-8");
        let get = |name: &str| headers.iter().find(|(k, _)| *k == name).map(|(_, v)| v.clone());
        assert_eq!(get("content-security-policy").as_deref(), Some(HTML_CSP));
        assert_eq!(get("x-content-type-options").as_deref(), Some("nosniff"));
        assert_eq!(get("content-disposition"), None);
        assert!(HTML_CSP.starts_with("sandbox allow-scripts;"));
    }

    #[test]
    fn html_reports_may_style_and_show_images_but_stay_sandboxed() {
        let policy = HTML_CSP;
        // The sandbox stays first and without allow-same-origin: the page is an opaque origin.
        assert!(policy.starts_with("sandbox allow-scripts;"));
        assert!(!policy.contains("allow-same-origin"));
        assert!(!policy.contains("allow-top-navigation"));
        // The directives the reports need: inline styles, and images from data:, blob: and https:.
        let directive = |name: &str| policy.split(';').map(str::trim).find(|d| d.starts_with(name)).map(str::to_string);
        assert_eq!(directive("style-src").as_deref(), Some("style-src 'self' 'unsafe-inline' https:"));
        assert_eq!(directive("img-src").as_deref(), Some("img-src 'self' data: blob: https:"));
        // Scripts keep their own rule, unchanged.
        assert_eq!(directive("script-src").as_deref(), Some("script-src 'unsafe-inline' 'self' https:"));
        // The policy is served with HTML, and nothing else gets it.
        let (_, headers) = served("text/html", "report.html", false);
        assert!(headers.iter().any(|(k, v)| *k == "content-security-policy" && v == policy));
    }

    #[test]
    fn an_html_name_is_html_whatever_the_stored_type() {
        let (kind, headers) = served("application/octet-stream", "Report.HTM", false);
        assert_eq!(kind, "text/html; charset=utf-8");
        assert!(headers.iter().any(|(k, _)| *k == "content-security-policy"));
        let (kind, _) = served("text/html", "x.bin", false);
        assert_eq!(kind, "text/html; charset=utf-8");
    }

    #[test]
    fn svg_downloads_and_other_files_carry_no_policy() {
        let (kind, headers) = served("image/svg+xml", "logo.svg", false);
        assert_eq!(kind, "image/svg+xml");
        assert!(headers.iter().any(|(k, v)| *k == "content-disposition" && v == "attachment; filename=\"logo.svg\""));
        assert!(!headers.iter().any(|(k, _)| *k == "content-security-policy"));
        let (kind, headers) = served("image/png", "shot.png", false);
        assert_eq!(kind, "image/png");
        assert_eq!(headers, vec![("x-content-type-options", "nosniff".to_string())]);
        let (_, headers) = served("text/html", "page.html", true);
        assert!(headers.iter().any(|(k, _)| *k == "content-disposition"));
    }

    #[test]
    fn content_type_comes_from_the_extension() {
        assert_eq!(guess_type("clip.MP4"), "video/mp4");
        assert_eq!(guess_type("notes.md"), "text/markdown; charset=utf-8");
        assert_eq!(guess_type("blob"), "application/octet-stream");
        assert_eq!(guess_type("data.unknownext"), "application/octet-stream");
    }

    #[test]
    fn private_files_are_readable_by_author_and_grantees_only() {
        let dir = scratch("access");
        let store = Store::load(&dir, &BTreeMap::new(), vault_in(&dir)).unwrap();
        let mut up = upload("secret.txt");
        up.author_cid = 1;
        up.audience = "private".to_string();
        up.allowed = vec![2];
        let item = store.add(up, b"private bytes").unwrap();
        assert!(store.can_read(&item.id, 1));
        assert!(store.can_read(&item.id, 2));
        assert!(!store.can_read(&item.id, 3));
        assert!(store.grant(&item.id, 3, &[3]).is_err());
        store.grant(&item.id, 1, &[3]).unwrap();
        assert!(store.can_read(&item.id, 3));
        assert!(store.publish(&item.id, 2).is_err());
        store.publish(&item.id, 1).unwrap();
        assert!(store.can_read(&item.id, 9));
        assert!(store.meta(&item.id).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
