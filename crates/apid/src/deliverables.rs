//! The Library: deliverables agents hand in (files they were asked to produce), kept in the hub's object store (apz-blobs),
//! by BLAKE3 hash.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use apz_blobs::Hash;
use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::internal::authorized;
use crate::state::App;

/// The longest name a deliverable may carry.
const NAME_CAP: usize = 120;

/// One file an agent handed in: a row of `deliverable` (its `allowed` agents are rows of `deliverable_allowed`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "deliverable")]
pub struct Deliverable {
    #[table(key)]
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
    /// The bytes' BLAKE3 hash (lower-case hex): where the object store holds them.
    pub hash: String,
    /// "deliverable" (handed in with deliver_file) or "shared" (share_file).
    #[serde(default = "deliverable_kind")]
    pub kind: String,
    /// "public" (every agent, the board) or "private" (author, `allowed`, the owner).
    #[serde(default = "public_audience")]
    pub audience: String,
    /// Agents a private file is shared with.
    #[serde(default)]
    #[table(list)]
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
    /// The hub's object store: the bytes of every file, by hash.
    blobs: Arc<apz_blobs::Blobs>,
    /// Every deliverable, oldest first. The saver keeps it in `plaza.db` (see `rows` and `load`).
    index: Mutex<Vec<Deliverable>>,
}

impl Store {
    /// The file's record, if it exists.
    pub fn meta(&self, id: &str) -> Option<Deliverable> {
        self.find(id)
    }

    /// Whether agent `cid` may read file `id` (its author, a public file, or one shared with it).
    pub fn can_read(&self, id: &str, cid: u64) -> bool {
        self.find(id).is_some_and(|d| {
            d.audience == "public" || d.author_cid == cid || d.allowed.contains(&cid)
        })
    }

    /// Makes file `id` public (attached to a board post or comment). Only its author may.
    pub fn publish(&self, id: &str, by: u64) -> Result<(), String> {
        let mut index = lock(&self.index);
        let item = index
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or_else(|| "no such file".to_string())?;
        if item.audience == "public" {
            return Ok(());
        }
        if by != item.author_cid {
            return Err("only the file's author can make it public".to_string());
        }
        item.audience = "public".to_string();
        Ok(())
    }

    /// Gives agents `to` access to file `id` (attached to a direct message). Only its author may.
    pub fn grant(&self, id: &str, by: u64, to: &[u64]) -> Result<(), String> {
        let mut index = lock(&self.index);
        let item = index
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or_else(|| "no such file".to_string())?;
        if item.audience == "public" {
            return Ok(());
        }
        if by != item.author_cid {
            return Err("only the file's author can share it".to_string());
        }
        for cid in to {
            if !item.allowed.contains(cid) {
                item.allowed.push(*cid);
            }
        }
        Ok(())
    }

    /// Opens the library over the object store, empty until `load` fills it from `plaza.db` at start.
    pub fn open(blobs: Arc<apz_blobs::Blobs>) -> Self {
        Self {
            blobs,
            index: Mutex::new(Vec::new()),
        }
    }

    /// Stores the bytes and indexes them. The bytes go first, so an index entry never names a missing blob. The index
    /// is saved by the saver (an owner's upload is saved at once: `owner_put_route`).
    pub fn add(&self, upload: Upload, body: &[u8]) -> Result<Deliverable, String> {
        let hash = self.blobs.put(body)?;
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
            hash: hash.to_string(),
            kind: upload.kind,
            audience: upload.audience,
            allowed: upload.allowed,
        };
        lock(&self.index).push(item.clone());
        Ok(item)
    }

    /// Every deliverable, newest first.
    pub fn list(&self) -> Vec<Deliverable> {
        lock(&self.index).iter().rev().cloned().collect()
    }

    pub fn find(&self, id: &str) -> Option<Deliverable> {
        lock(&self.index).iter().find(|d| d.id == id).cloned()
    }

    /// The bytes of one deliverable, from the object store.
    pub fn read_blob(&self, item: &Deliverable) -> Result<Vec<u8>, String> {
        self.blobs.get(&item.hash.parse::<Hash>()?)
    }

    /// The hashes of every file's bytes: the sweep keeps them (storage.rs).
    pub fn hashes(&self) -> Vec<Hash> {
        lock(&self.index)
            .iter()
            .filter_map(|d| d.hash.parse().ok())
            .collect()
    }

    /// Removes a deliverable's entry, returning it, or None when unknown. Its bytes stay in the object store until the
    /// sweep finds that nothing names them.
    pub fn remove(&self, id: &str) -> Option<Deliverable> {
        let mut index = lock(&self.index);
        let pos = index.iter().position(|d| d.id == id)?;
        Some(index.remove(pos))
    }
}

/// `/v1/files…` (the console) and `/internal/v1/agents/{cid}/gateway/files…` (agents, via
/// the hub's plaza.gateway).
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/files", list_route)
        .doc(RouteDoc::new("List the files agents handed in"))
        .get("/v1/files/{id}", meta_route)
        .doc(RouteDoc::new("Get a file's metadata").errors(&[404]))
        .get("/v1/files/{id}/content", content_route)
        .doc(RouteDoc::new("Get a file's content").errors(&[404]))
        .delete("/v1/files/{id}", delete_route)
        .doc(RouteDoc::new("Delete a file").errors(&[404, 500]))
        .route("PUT", "/internal/v1/agents/{cid}/gateway/files", put_route)
        .doc(
            RouteDoc::new("Store a file an agent hands in")
                .status(201)
                .errors(&[400, 403]),
        )
        .get(
            "/internal/v1/agents/{cid}/gateway/files/{id}",
            agent_get_route,
        )
        .doc(RouteDoc::new("Let an agent download a file it can read").errors(&[404]))
        .route("PUT", "/v1/files", owner_put_route)
        .doc(
            RouteDoc::new("Upload a file of the owner's to attach to a message (?name=)")
                .status(201)
                .errors(&[400, 413, 500]),
        )
}

/// The owner's attachments: at most this big.
const MAX_ATTACHMENT: usize = 100 << 20;

/// `PUT /v1/files?name=&title=`: a file the owner attaches to a chat or a conversation. It is private (only the
/// owner reads it) until a message carries it: then the agent it goes to may read it ([`attach`]).
fn owner_put_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
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
    if req.body.len() > MAX_ATTACHMENT {
        return Response::error(
            413,
            &format!("an attachment is at most {} MiB", MAX_ATTACHMENT >> 20),
        );
    }
    let upload = Upload {
        kind: "attachment".to_string(),
        audience: "private".to_string(),
        allowed: Vec::new(),
        title: query(req, "title").unwrap_or_else(|| name.clone()),
        description: None,
        task: None,
        content_type: optional(req.header("content-type"))
            .filter(|t| t != "application/octet-stream")
            .unwrap_or_else(|| guess_type(&name).to_string()),
        author: "owner".to_string(),
        author_cid: 0,
        name,
    };
    match app.deliverables.add(upload, &req.body) {
        Ok(item) => {
            crate::store::saver::save_now(app, "deliverables");
            app.bus.publish("file", &item);
            Response::json(201, &item)
        }
        Err(e) => Response::error(500, &e),
    }
}

/// The owner's attachments `ids` go with a message to agent `cid`: the agent may read each from now on. Answers the
/// lines the message's prompt carries about them, or why one cannot go.
pub fn attach(app: &App, ids: &[String], cid: u64) -> Result<String, String> {
    if ids.is_empty() {
        return Ok(String::new());
    }
    if ids.len() > 20 {
        return Err("at most 20 attachments a message".into());
    }
    let mut lines = vec!["Attached files (read one with plaza_get_file, or download http://plaza.gateway/files/<id>):".to_string()];
    for id in ids {
        let meta = app
            .deliverables
            .meta(id)
            .ok_or_else(|| format!("no file {id}"))?;
        if meta.author_cid != 0 {
            return Err(format!("file {id} is not the owner's"));
        }
        app.deliverables.grant(id, 0, &[cid])?;
        lines.push(format!(
            "- {} ({}, {} bytes): id {}",
            meta.title, meta.content_type, meta.size, meta.id
        ));
    }
    Ok(lines.join("\n"))
}

/// The attachments as the console shows them on a message: `{id, name, size, type}` each.
pub fn attachment_views(app: &App, ids: &[String]) -> Vec<crate::conversations::Attachment> {
    ids.iter()
        .filter_map(|id| app.deliverables.meta(id))
        .map(|m| crate::conversations::Attachment {
            id: m.id,
            name: m.title,
            size: m.size,
            mime: m.content_type,
        })
        .collect()
}

/// Every deliverable, or only one `kind` when the query names it.
fn list_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let kind = query(req, "kind");
    let items: Vec<Deliverable> = app
        .deliverables
        .list()
        .into_iter()
        .filter(|d| kind.as_deref().is_none_or(|k| d.kind == k))
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
        Some(item) => serve(app, req, &item, req.query_param("download") == Some("1")),
        None => Response::error(404, "no such file"),
    }
}

fn delete_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    match app.deliverables.remove(p.get("id")) {
        Some(item) => {
            crate::store::saver::save_now(app, "deliverables");
            app.bus
                .publish("file", &json!({ "id": item.id, "deleted": true }));
            Response::json(200, &json!({ "deleted": item.id }))
        }
        None => Response::error(404, "no such file"),
    }
}

fn put_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    authorized(app, req, p.get("cid"), |cid| {
        if app.agents.get(cid).is_none() {
            return Err(Response::error(404, "unknown agent"));
        }
        Ok(put_file(app, req, cid))
    })
}

fn agent_get_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    authorized(app, req, p.get("cid"), |cid| {
        Ok(match app.deliverables.find(p.get("id")) {
            Some(item) if app.deliverables.can_read(&item.id, cid) => serve(app, req, &item, false),
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
    let role = app
        .agents
        .get(cid)
        .map(|agent| agent.role)
        .unwrap_or_default();
    let content = serde_json::Value::String(String::from_utf8_lossy(&req.body).into_owned());
    let artifact = jhrl::authz::Entity {
        ty: "Artifact".into(),
        id: name.clone(),
    };
    if let Some(why) = app.laws.refuse_op(
        cid,
        &role,
        "artifact.publish",
        artifact,
        BTreeMap::new(),
        &content,
    ) {
        return Response::error(403, &why);
    }
    let kind = if query(req, "kind").as_deref() == Some("shared") {
        "shared"
    } else {
        "deliverable"
    };
    let to = match parse_cids(query(req, "to").as_deref()) {
        Ok(to) => to,
        Err(e) => return Response::error(400, &e),
    };
    let (audience, allowed) = if to.is_empty() {
        ("public", Vec::new())
    } else {
        ("private", to)
    };
    let upload = Upload {
        kind: kind.to_string(),
        audience: audience.to_string(),
        allowed,
        title: query(req, "title").unwrap_or_else(|| name.clone()),
        description: query(req, "description"),
        task: query(req, "task"),
        content_type: optional(req.header("content-type"))
            .unwrap_or_else(|| guess_type(&name).to_string()),
        author: app
            .agents
            .get(cid)
            .map(|a| a.label())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| format!("agent {cid}")),
        author_cid: cid,
        name,
    };
    match app.deliverables.add(upload, &req.body) {
        Ok(item) => {
            crate::store::saver::save_now(app, "deliverables");
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
/// No `connect-src`, `form-action` or `base-uri` but `'none'`: the page is served from the hub's own origin, so a
/// request it made would reach the hub's API; it may show itself, not call anything.
const HTML_CSP: &str = "sandbox allow-scripts; default-src 'self' data: blob: https:; script-src 'unsafe-inline' 'self' https:; style-src 'self' 'unsafe-inline' https:; img-src 'self' data: blob: https:; connect-src 'none'; form-action 'none'; base-uri 'none'";

/// A file is HTML when its content type says so or its name ends `.html` or `.htm`.
fn is_html(content_type: &str, name: &str) -> bool {
    let ext = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase());
    content_type.starts_with("text/html") || matches!(ext.as_deref(), Some("html" | "htm"))
}

/// What a stored file is served as: its content type and its extra headers. HTML is served inline as text/html under
/// `HTML_CSP`, so the console's frame and a direct open both run it sandboxed. SVG (which can carry script) always
/// downloads, and `download=1` downloads anything.
pub(crate) fn served(
    content_type: &str,
    name: &str,
    download: bool,
) -> (String, Vec<(&'static str, String)>) {
    let html = is_html(content_type, name);
    let served_type = if html {
        "text/html; charset=utf-8".to_string()
    } else {
        content_type.to_string()
    };
    let mut headers = vec![("x-content-type-options", "nosniff".to_string())];
    if html {
        headers.push(("content-security-policy", HTML_CSP.to_string()));
    }
    if download || content_type.contains("svg") {
        headers.push((
            "content-disposition",
            format!("attachment; filename=\"{name}\""),
        ));
    }
    (served_type, headers)
}

/// The bytes with their content type and headers (see `served`), a Range request included (`storage::blob_response`).
fn serve(app: &App, req: &Request, item: &Deliverable, download: bool) -> Response {
    let (content_type, headers) = served(&item.content_type, &item.name, download);
    crate::storage::blob_response(&app.blobs, req, &item.hash, &content_type, headers)
}

/// The name as stored: its basename, with anything outside `[A-Za-z0-9._-]` made `_`. A name
/// that is empty, only dots, or longer than 120 characters is refused.
pub fn clean_name(raw: &str) -> Result<String, String> {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let name: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
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
    let Some((_, ext)) = name.rsplit_once('.') else {
        return "application/octet-stream";
    };
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

/// `f` and twelve hex digits from the OS random source (the clock if that is unreadable).
fn new_id() -> String {
    let mut bytes = [0u8; 6];
    let random = apz_os::random::fill(&mut bytes);
    if random.is_err() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        bytes.copy_from_slice(&nanos.to_be_bytes()[10..]);
    }
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("f{hex}")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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
                match u8::from_str_radix(
                    std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("zz"),
                    16,
                ) {
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
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

/// The agent cids a `to` query lists, comma-separated (`203,207`); absent is none.
fn parse_cids(raw: Option<&str>) -> Result<Vec<u64>, String> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    raw.split(',')
        .map(|s| {
            s.trim()
                .parse::<u64>()
                .map_err(|_| "to must list agent cids".to_string())
        })
        .collect()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl Store {
    /// The library's rows, as its index is now (the saver writes what changed).
    pub fn rows(&self) -> Result<apz_store::RowSet, apz_store::StoreError> {
        let items = lock(&self.index);
        apz_store::RowSet::list(def(), items.iter())
    }

    /// Replaces the index with what `plaza.db` holds (at start).
    pub fn load(
        &self,
        conn: &apz_store::rusqlite::Connection,
    ) -> Result<(), apz_store::StoreError> {
        let items: Vec<Deliverable> = apz_store::table::load_list(conn, def())?;
        *lock(&self.index) = items;
        Ok(())
    }
}

/// The library's tables: `deliverable`, with the agents a private file is shared with as `deliverable_allowed`.
pub fn def() -> &'static apz_store::TableDef {
    apz_store::root_def!(Deliverable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// The object store in `<dir>/blobs`, as the hub opens it at start.
    fn blobs_in(dir: &Path) -> Arc<apz_blobs::Blobs> {
        Arc::new(apz_blobs::Blobs::open(&dir.join("blobs")).expect("the object store opens"))
    }

    #[test]
    fn query_values_are_percent_decoded() {
        assert_eq!(decode("Hello%20bottle+ad"), "Hello bottle ad");
        assert_eq!(decode("100%25"), "100%");
        assert_eq!(decode("50%"), "50%");
        assert_eq!(decode("%E2%9C%93"), "\u{2713}");
    }

    #[test]
    fn rows_load_back_whole_from_their_tables() {
        let a = Store::open(blobs_in(&scratch("rows")));
        a.add(upload("report.txt"), b"hello").unwrap();
        let mut private = upload("notes.md");
        private.description = Some("Working notes".to_string());
        private.audience = "private".to_string();
        private.allowed = vec![2, 3];
        a.add(private, b"private bytes").unwrap();
        // The index is the rows in `plaza.db`, not a file.
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &a.rows().unwrap()).unwrap();
        let b = Store::open(blobs_in(&scratch("rows-restored")));
        b.load(&conn).unwrap();
        assert_eq!(
            serde_json::to_value(b.list()).unwrap(),
            serde_json::to_value(a.list()).unwrap()
        );
        let listed = b.list();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].description.as_deref(), Some("Working notes"));
        assert_eq!(listed[0].allowed, vec![2, 3]);
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("apid-deliverables-{}-{name}", std::process::id()));
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
        let store = Store::open(blobs_in(&dir));
        let body = b"hello deliverable".to_vec();
        let item = store.add(upload("report.txt"), &body).unwrap();
        assert!(item.id.starts_with('f') && item.id.len() == 13);
        assert_eq!(item.size, body.len() as u64);
        // The row names the bytes by their BLAKE3 hash, and the object store holds them under it.
        assert_eq!(item.hash, Hash::of(&body).to_string());
        assert_eq!(store.list().len(), 1);
        let found = store.find(&item.id).unwrap();
        assert_eq!(store.read_blob(&found).unwrap(), body);
        assert_eq!(store.hashes(), vec![Hash::of(&body)]);
        // A restart loads the rows the saver wrote, over the same object store.
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &store.rows().unwrap()).unwrap();
        drop(store);
        let reloaded = Store::open(blobs_in(&dir));
        reloaded.load(&conn).unwrap();
        assert_eq!(reloaded.list()[0].id, item.id);
        assert_eq!(reloaded.read_blob(&item).unwrap(), body);
        // A removal drops the entry; its bytes stay until the sweep finds that nothing names them.
        assert!(reloaded.remove(&item.id).is_some());
        assert!(reloaded.list().is_empty());
        assert!(reloaded.find(&item.id).is_none());
        assert!(reloaded.hashes().is_empty());
        assert_eq!(reloaded.blobs.get(&Hash::of(&body)).unwrap(), body);
        // The removal is saved as a removal.
        apz_store::table::replace(&conn, def(), &reloaded.rows().unwrap()).unwrap();
        drop(reloaded);
        let emptied = Store::open(blobs_in(&dir));
        emptied.load(&conn).unwrap();
        assert!(emptied.list().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_are_basenames_of_safe_characters() {
        assert_eq!(clean_name("../../etc/passwd").unwrap(), "passwd");
        assert_eq!(
            clean_name("C:\\out\\my report (final).pdf").unwrap(),
            "my_report__final_.pdf"
        );
        assert!(clean_name("..").is_err());
        assert!(clean_name("").is_err());
        assert!(clean_name(&"a".repeat(121)).is_err());
        assert_eq!(clean_name(&"a".repeat(120)).unwrap().len(), 120);
    }

    #[test]
    fn served_policy_and_headers_follow_the_file_type() {
        // HTML reports may style and show images but stay sandboxed.
        {
            let policy = HTML_CSP;
            // The sandbox stays first and without allow-same-origin: the page is an opaque origin.
            assert!(policy.starts_with("sandbox allow-scripts;"));
            assert!(!policy.contains("allow-same-origin"));
            assert!(!policy.contains("allow-top-navigation"));
            // The directives the reports need: inline styles, and images from data:, blob: and https:.
            let directive = |name: &str| {
                policy
                    .split(';')
                    .map(str::trim)
                    .find(|d| d.starts_with(name))
                    .map(str::to_string)
            };
            assert_eq!(
                directive("style-src").as_deref(),
                Some("style-src 'self' 'unsafe-inline' https:")
            );
            assert_eq!(
                directive("img-src").as_deref(),
                Some("img-src 'self' data: blob: https:")
            );
            // Scripts keep their own rule, unchanged.
            assert_eq!(
                directive("script-src").as_deref(),
                Some("script-src 'unsafe-inline' 'self' https:")
            );
            // The page reaches nothing: it is served from the hub's origin, whose API it must not call.
            assert_eq!(
                directive("connect-src").as_deref(),
                Some("connect-src 'none'")
            );
            assert_eq!(
                directive("form-action").as_deref(),
                Some("form-action 'none'")
            );
            // The policy is served with HTML, inline, with nosniff and no download header; nothing else gets it.
            let (kind, headers) = served("text/html; charset=utf-8", "report.html", false);
            assert_eq!(kind, "text/html; charset=utf-8");
            let get = |name: &str| {
                headers
                    .iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| v.clone())
            };
            assert_eq!(get("content-security-policy").as_deref(), Some(policy));
            assert_eq!(get("x-content-type-options").as_deref(), Some("nosniff"));
            assert_eq!(get("content-disposition"), None);
        }
        // An HTML name is HTML whatever the stored type.
        {
            let (kind, headers) = served("application/octet-stream", "Report.HTM", false);
            assert_eq!(kind, "text/html; charset=utf-8");
            assert!(headers.iter().any(|(k, _)| *k == "content-security-policy"));
            let (kind, _) = served("text/html", "x.bin", false);
            assert_eq!(kind, "text/html; charset=utf-8");
        }
        // SVG downloads and other files carry no policy.
        {
            let (kind, headers) = served("image/svg+xml", "logo.svg", false);
            assert_eq!(kind, "image/svg+xml");
            assert!(
                headers.iter().any(|(k, v)| *k == "content-disposition"
                    && v == "attachment; filename=\"logo.svg\"")
            );
            assert!(!headers.iter().any(|(k, _)| *k == "content-security-policy"));
            let (kind, headers) = served("image/png", "shot.png", false);
            assert_eq!(kind, "image/png");
            assert_eq!(
                headers,
                vec![("x-content-type-options", "nosniff".to_string())]
            );
            let (_, headers) = served("text/html", "page.html", true);
            assert!(headers.iter().any(|(k, _)| *k == "content-disposition"));
        }
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
        let store = Store::open(blobs_in(&dir));
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
