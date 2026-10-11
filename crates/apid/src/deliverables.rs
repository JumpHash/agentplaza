//! The Library: deliverables agents hand in (files they were asked to produce), kept in the
//! plaza's object store — Garage over S3 when configured, a local folder otherwise.

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use apz_gateway::s3::S3;
use apz_http::{Params, Request, Response, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::internal::authorized;
use crate::providers::write_atomic;
use crate::state::App;

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
}

pub struct Store {
    /// `<state-dir>/files`: the index, and the blobs when they are kept locally.
    dir: PathBuf,
    /// Garage, when every `APZ_S3_*` setting is present.
    s3: Option<S3>,
    index: Mutex<Vec<Deliverable>>,
}

impl Store {
    /// Loads `<state-dir>/files/index.json` (a missing file is an empty library) and picks
    /// the store: Garage when the environment configures it, else a local folder.
    pub fn load(state_dir: &Path, env: &BTreeMap<String, String>) -> io::Result<Self> {
        let dir = state_dir.join("files");
        std::fs::create_dir_all(&dir)?;
        let index: Vec<Deliverable> = match std::fs::read(dir.join(INDEX)) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        Ok(Self { dir, s3: s3_from(env), index: Mutex::new(index) })
    }

    /// Stores the bytes and indexes them. The bytes go first, so an index entry never names
    /// a missing object; a failed index save removes them again.
    pub fn add(&self, upload: Upload, body: &[u8]) -> Result<Deliverable, String> {
        let store = if self.s3.is_some() { "garage" } else { "local" };
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

    fn garage(&self) -> Result<&S3, String> {
        self.s3
            .as_ref()
            .ok_or_else(|| "the file is in Garage, which is not configured".to_string())
    }

    fn save(&self, index: &[Deliverable]) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(index)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        write_atomic(&self.dir.join(INDEX), &bytes, 0o600)
    }

    fn blob_path(&self, id: &str) -> PathBuf {
        self.dir.join("blobs").join(id)
    }

    fn write_blob(&self, item: &Deliverable, body: &[u8]) -> Result<(), String> {
        match &self.s3 {
            Some(s3) => s3.put(&object_key(item), body, &item.content_type),
            None => {
                std::fs::create_dir_all(self.dir.join("blobs")).map_err(|e| e.to_string())?;
                write_atomic(&self.blob_path(&item.id), body, 0o600).map_err(|e| e.to_string())
            }
        }
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
        .get("/v1/files/{id}/content", content_route)
        .delete("/v1/files/{id}", delete_route)
        .route("PUT", "/internal/v1/agents/{cid}/gateway/files", put_route)
        .get("/internal/v1/agents/{cid}/gateway/files/{id}", agent_get_route)
}

fn list_route(app: &App, _: &Request, _: &Params<'_>) -> Response {
    Response::json(200, &app.deliverables.list())
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
    authorized(app, req, p.get("cid"), |_| {
        Ok(match app.deliverables.find(p.get("id")) {
            Some(item) => serve(app, &item, false),
            None => Response::error(404, "no such file"),
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
    let upload = Upload {
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
                &json!({ "id": item.id, "url": format!("http://plaza.gateway/files/{}", item.id), "size": item.size }),
            )
        }
        Err(e) => Response::error(500, &e),
    }
}

/// The bytes with their content type. Markup and SVG run script when opened, so they always
/// download; `download=1` asks for a download of anything.
fn serve(app: &App, item: &Deliverable, download: bool) -> Response {
    let bytes = match app.deliverables.read_blob(item) {
        Ok(bytes) => bytes,
        Err(e) => return Response::error(500, &e),
    };
    let risky = item.content_type.contains("html") || item.content_type.contains("svg");
    let response = Response::new(200, &item.content_type, bytes).with_header("x-content-type-options", "nosniff");
    if download || risky {
        response.with_header("content-disposition", &format!("attachment; filename=\"{}\"", item.name))
    } else {
        response
    }
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

/// Garage, when every `APZ_S3_*` setting is present in the env file or the process.
fn s3_from(env: &BTreeMap<String, String>) -> Option<S3> {
    let get = |key: &str| {
        env.get(key)
            .cloned()
            .or_else(|| std::env::var(key).ok())
            .filter(|v| !v.is_empty())
    };
    Some(S3 {
        endpoint: get("APZ_S3_ENDPOINT")?,
        bucket: get("APZ_S3_BUCKET")?,
        region: get("APZ_S3_REGION").unwrap_or_else(|| "garage".to_string()),
        access_key: get("APZ_S3_ACCESS_KEY")?,
        secret_key: get("APZ_S3_SECRET_KEY")?,
    })
}

/// `f` and twelve hex digits from the OS random source (the clock if that is unreadable).
fn new_id() -> String {
    let mut bytes = [0u8; 6];
    let random = std::fs::File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut bytes));
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

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_values_are_percent_decoded() {
        assert_eq!(decode("Hello%20bottle+ad"), "Hello bottle ad");
        assert_eq!(decode("100%25"), "100%");
        assert_eq!(decode("50%"), "50%");
        assert_eq!(decode("%E2%9C%93"), "\u{2713}");
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
        }
    }

    #[test]
    fn put_list_get_delete_round_trip() {
        let dir = scratch("roundtrip");
        let env = BTreeMap::new();
        let store = Store::load(&dir, &env).unwrap();
        let body = b"hello deliverable".to_vec();
        let item = store.add(upload("report.txt"), &body).unwrap();
        assert!(item.id.starts_with('f') && item.id.len() == 13);
        assert_eq!(item.store, "local");
        assert_eq!(item.size, body.len() as u64);
        assert_eq!(item.sha256, sha256_hex(&body));
        assert_eq!(store.list().len(), 1);
        let found = store.find(&item.id).unwrap();
        assert_eq!(store.read_blob(&found).unwrap(), body);
        let reloaded = Store::load(&dir, &env).unwrap();
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
    fn content_type_comes_from_the_extension() {
        assert_eq!(guess_type("clip.MP4"), "video/mp4");
        assert_eq!(guess_type("notes.md"), "text/markdown; charset=utf-8");
        assert_eq!(guess_type("blob"), "application/octet-stream");
        assert_eq!(guess_type("data.unknownext"), "application/octet-stream");
    }
}
