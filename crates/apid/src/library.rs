//! The plaza's library of agent-made tools. An agent packages a program with the
//! `tool_package` tool and the plaza keeps it as the next version of a named tool.
//! A new version is private to its author until the owner approves it; approved
//! versions are offered to every agent as `x_<name>` tools and run as guest programs.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use apz_http::{Request, Response, RouteDoc, Router};
use apz_proto::chat::ToolSpec;
use apz_proto::wire::{Program, ProgramFile};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::state::App;

/// The prefix that marks a library tool's name.
pub const PREFIX: &str = "x_";
/// The largest file a package may carry, in bytes.
const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
/// The most text a file view returns, in bytes.
const VIEW_BYTES: u64 = 256 * 1024;
/// The wall-clock limit of a library program, in seconds.
const TIMEOUT_S: u64 = 120;

fn default_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

/// One file of a stored version (a row of the entry's `library_entry_files` child table).
#[derive(Clone, Debug, Serialize, Deserialize, apz_store::Table)]
pub struct FileInfo {
    pub path: String,
    pub size: u64,
    #[serde(default)]
    pub executable: bool,
}

/// One stored version of a library tool: a row of `library_entry`, keyed by its name and version.
#[derive(Clone, Debug, Serialize, Deserialize, apz_store::Table)]
#[table(name = "library_entry")]
pub struct Entry {
    #[table(key)]
    pub name: String,
    #[table(key)]
    pub version: u32,
    pub author_cid: u64,
    pub author: String,
    pub description: String,
    #[table(list)]
    pub command: Vec<String>,
    #[serde(default)]
    #[table(list)]
    pub requires: Vec<String>,
    /// The tool's JSON Schema, kept as written.
    #[table(content)]
    pub input_schema: Value,
    /// "private", "pending", "published" or "rejected".
    pub state: String,
    #[table(child)]
    pub files: Vec<FileInfo>,
    pub hash: String,
    pub created_ms: u64,
    #[serde(default)]
    pub calls: u64,
    #[serde(default)]
    pub review_note: String,
}

/// A package as the guest reports it from `tool_package`.
#[derive(Clone, Debug, Deserialize)]
pub struct Package {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default = "default_schema")]
    pub input_schema: Value,
    #[serde(default)]
    pub publish: bool,
    pub files: Vec<PackageFile>,
}

/// One file of a [`Package`], its bytes base64-encoded.
#[derive(Clone, Debug, Deserialize)]
pub struct PackageFile {
    pub path: String,
    pub content_b64: String,
    #[serde(default)]
    pub executable: bool,
}

#[derive(Deserialize)]
struct Report {
    package: Package,
}

/// The library: its entries in memory, kept in the hub's store (`library_entry`). The tool files stay under
/// `<state>/library/<name>/<version>/files`.
pub struct Store {
    root: PathBuf,
    entries: Mutex<Vec<Entry>>,
}

impl Store {
    /// An empty library whose tool files live under `<state_dir>/library`; [`Store::load`] fills it at start.
    pub fn open(state_dir: impl AsRef<Path>) -> Store {
        Store {
            root: state_dir.as_ref().join("library"),
            entries: Mutex::new(Vec::new()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Vec<Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn files_dir(&self, name: &str, version: u32) -> PathBuf {
        self.root.join(name).join(version.to_string()).join("files")
    }

    /// Stores `package` as the next version of its name, private or pending review.
    pub fn insert(&self, author_cid: u64, author: &str, package: Package) -> Result<Entry, String> {
        validate_name(&package.name)?;
        if package.command.is_empty() {
            return Err("a package needs a command".to_string());
        }
        if package.files.is_empty() {
            return Err("a package needs at least one file".to_string());
        }
        let mut blobs: Vec<(String, Vec<u8>)> = Vec::new();
        let mut files = Vec::new();
        for file in &package.files {
            check_rel_path(&file.path)?;
            let bytes = base64_decode(&file.content_b64)
                .ok_or_else(|| format!("file {} is not base64", file.path))?;
            if bytes.len() > MAX_FILE_BYTES {
                return Err(format!("file {} is too large", file.path));
            }
            files.push(FileInfo {
                path: file.path.clone(),
                size: bytes.len() as u64,
                executable: file.executable,
            });
            blobs.push((file.path.clone(), bytes));
        }
        let hash = digest(&blobs);

        let mut entries = self.lock();
        let version = entries
            .iter()
            .filter(|e| e.name == package.name)
            .map(|e| e.version)
            .max()
            .unwrap_or(0)
            + 1;
        let dir = self.files_dir(&package.name, version);
        for (file, (_, bytes)) in package.files.iter().zip(&blobs) {
            let target = dir.join(&file.path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mode = if file.executable { 0o755 } else { 0o644 };
            crate::providers::write_atomic(&target, bytes, mode).map_err(|e| e.to_string())?;
        }
        let entry = Entry {
            name: package.name.clone(),
            version,
            author_cid,
            author: author.to_string(),
            description: package.description.clone(),
            command: package.command.clone(),
            requires: package.requires.clone(),
            input_schema: package.input_schema.clone(),
            state: if package.publish {
                "pending"
            } else {
                "private"
            }
            .to_string(),
            files,
            hash,
            created_ms: now_ms(),
            calls: 0,
            review_note: String::new(),
        };
        entries.push(entry.clone());
        Ok(entry)
    }

    /// Approves or rejects the pending version; returns its new state.
    pub fn review(
        &self,
        name: &str,
        version: u32,
        approve: bool,
        note: &str,
    ) -> Result<String, String> {
        let mut entries = self.lock();
        let entry = entries
            .iter_mut()
            .find(|e| e.name == name && e.version == version)
            .ok_or_else(|| "no such version".to_string())?;
        if entry.state != "pending" {
            return Err(format!("v{version} is {}, not pending", entry.state));
        }
        entry.state = if approve { "published" } else { "rejected" }.to_string();
        entry.review_note = note.to_string();
        Ok(entry.state.clone())
    }

    /// Removes one version and its files.
    pub fn delete(&self, name: &str, version: u32) -> Result<(), String> {
        let mut entries = self.lock();
        let before = entries.len();
        entries.retain(|e| !(e.name == name && e.version == version));
        if entries.len() == before {
            return Err("no such version".to_string());
        }
        let _ = fs::remove_dir_all(self.root.join(name).join(version.to_string()));
        Ok(())
    }

    /// Every stored version, newest first.
    pub fn all(&self) -> Vec<Entry> {
        let mut entries = self.lock().to_vec();
        entries.sort_by_key(|a| std::cmp::Reverse(a.created_ms));
        entries
    }

    /// Stored versions whose name, description or a file path contains `query` (case-insensitive), newest first.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Entry> {
        let needle = query.to_lowercase();
        self.all()
            .into_iter()
            .filter(|entry| {
                entry.name.to_lowercase().contains(&needle)
                    || entry.description.to_lowercase().contains(&needle)
                    || entry
                        .files
                        .iter()
                        .any(|file| file.path.to_lowercase().contains(&needle))
            })
            .take(limit)
            .collect()
    }

    /// The newest version of each name that agent `cid` may see.
    pub fn offered(&self, cid: u64) -> Vec<Entry> {
        latest(&self.lock(), cid)
    }

    /// The file list of one version, with text content up to `VIEW_BYTES` per file.
    pub fn view(&self, name: &str, version: u32) -> Result<Vec<Value>, String> {
        let entry = self
            .lock()
            .iter()
            .find(|e| e.name == name && e.version == version)
            .cloned()
            .ok_or_else(|| "no such version".to_string())?;
        let dir = self.files_dir(name, version);
        Ok(entry
            .files
            .iter()
            .map(|f| {
                let content = if f.size <= VIEW_BYTES { fs::read_to_string(dir.join(&f.path)).ok() } else { None };
                json!({ "path": f.path, "size": f.size, "executable": f.executable, "content": content })
            })
            .collect())
    }

    /// The program behind the visible tool `tool` (`x_<name>`), with its files; counts the call.
    pub fn program(&self, cid: u64, tool: &str) -> Option<Program> {
        let name = tool.strip_prefix(PREFIX)?;
        let mut entries = self.lock();
        let entry = entries
            .iter()
            .filter(|e| e.name == name && offerable(e, cid))
            .max_by_key(|e| e.version)?
            .clone();
        let idx = entries
            .iter()
            .position(|e| e.name == entry.name && e.version == entry.version)?;
        entries[idx].calls += 1;
        drop(entries);

        let dir = self.files_dir(&entry.name, entry.version);
        let files = entry
            .files
            .iter()
            .map(|f| {
                let bytes = fs::read(dir.join(&f.path)).ok()?;
                Some(ProgramFile {
                    path: f.path.clone(),
                    content_b64: base64_encode(&bytes),
                    executable: f.executable,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Program {
            name: entry.name,
            version: entry.version,
            hash: entry.hash,
            files,
            command: entry.command,
            requires: entry.requires,
            timeout_s: TIMEOUT_S,
        })
    }
}

/// A version agent `cid` may see: published ones for everyone, its own drafts for the author.
fn offerable(entry: &Entry, cid: u64) -> bool {
    entry.state == "published"
        || (entry.author_cid == cid && (entry.state == "private" || entry.state == "pending"))
}

/// The newest offerable version of each name, as agent `cid` sees the library.
fn latest(entries: &[Entry], cid: u64) -> Vec<Entry> {
    let mut best: BTreeMap<&str, &Entry> = BTreeMap::new();
    for entry in entries.iter().filter(|e| offerable(e, cid)) {
        match best.get(entry.name.as_str()) {
            Some(current) if current.version >= entry.version => {}
            _ => {
                best.insert(entry.name.as_str(), entry);
            }
        }
    }
    best.values().map(|e| (*e).clone()).collect()
}

/// The tool specs agent `cid` is offered from the library.
pub fn specs_for(app: &App, cid: u64) -> Vec<ToolSpec> {
    app.library
        .offered(cid)
        .into_iter()
        .map(|e| ToolSpec {
            name: format!("{PREFIX}{}", e.name),
            description: format!("{} (made by {}, v{})", e.description, e.author, e.version),
            input_schema: e.input_schema,
        })
        .collect()
}

/// The program behind a visible library tool, for the guest to run.
pub fn program(app: &App, cid: u64, name: &str) -> Option<Program> {
    app.library.program(cid, name)
}

/// Stores a package reported by a guest's `tool_package` call and announces it when pending.
pub fn accept(app: &App, cid: u64, output: &str) -> Result<Entry, String> {
    let report: Report = serde_json::from_str(output).map_err(|e| e.to_string())?;
    let author = app.agents.get(cid).map(|a| a.label()).unwrap_or_default();
    let entry = app.library.insert(cid, &author, report.package)?;
    announce(app, &entry.name, entry.version, &entry.state);
    // The author can use its new tool at once: its tool list is pushed again.
    if let Some(agent) = app.agents.get(cid) {
        crate::agents::push_profile(app, &agent);
    }
    Ok(entry)
}

fn announce(app: &App, name: &str, version: u32, state: &str) {
    app.bus.publish(
        "library",
        &json!({ "name": name, "version": version, "state": state }),
    );
}

type Calls = Mutex<HashMap<(u64, String), ()>>;

fn package_calls() -> &'static Calls {
    static CALLS: OnceLock<Calls> = OnceLock::new();
    CALLS.get_or_init(Calls::default)
}

/// Remembers that guest `cid` made the `tool_package` call `call`.
pub fn remember_package_call(cid: u64, call: &str) {
    package_calls()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert((cid, call.to_string()), ());
}

/// Whether `call` was a `tool_package` call; forgets it either way.
pub fn take_package_call(cid: u64, call: &str) -> bool {
    package_calls()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&(cid, call.to_string()))
        .is_some()
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/library", |app, _req, _p| list(app))
        .doc(RouteDoc::new("List the library's tool versions"))
        .get("/v1/library/{name}/{version}/files", |app, req, p| {
            files(
                app,
                p.get("name"),
                p.get("version"),
                req.query_param("path"),
            )
        })
        .doc(RouteDoc::new("List a tool version's files, or read one").errors(&[404]))
        .post("/v1/library/{name}/{version}/review", |app, req, p| {
            review(app, req, p.get("name"), p.get("version"))
        })
        .doc(RouteDoc::new("Approve or reject a tool version").errors(&[400, 409]))
        .delete("/v1/library/{name}/{version}", |app, _req, p| {
            delete(app, p.get("name"), p.get("version"))
        })
        .doc(RouteDoc::new("Delete a tool version").errors(&[404]))
}

#[derive(Deserialize)]
struct ReviewBody {
    approve: bool,
    #[serde(default)]
    note: String,
}

fn list(app: &App) -> Response {
    Response::json(200, &app.library.all())
}

/// A version's files; with `?path=`, just that file as `{path, content}` (what the console reads).
fn files(app: &App, name: &str, version: &str, path: Option<&str>) -> Response {
    let Ok(version) = version.parse::<u32>() else {
        return Response::error(400, "the version must be a number");
    };
    match (app.library.view(name, version), path) {
        (Ok(files), None) => Response::json(
            200,
            &json!({ "name": name, "version": version, "files": files }),
        ),
        (Ok(files), Some(path)) => match files
            .into_iter()
            .find(|f| f["path"].as_str() == Some(path))
        {
            Some(file) => Response::json(200, &json!({ "path": path, "content": file["content"] })),
            None => Response::error(404, &format!("no file {path} in {name} v{version}")),
        },
        (Err(e), _) => Response::error(404, &e),
    }
}

fn review(app: &App, req: &Request, name: &str, version: &str) -> Response {
    let Ok(version) = version.parse::<u32>() else {
        return Response::error(400, "the version must be a number");
    };
    let body: ReviewBody = match req.json() {
        Ok(body) => body,
        Err(e) => return Response::error(400, &e.to_string()),
    };
    match app.library.review(name, version, body.approve, &body.note) {
        Ok(state) => {
            announce(app, name, version, &state);
            // A published tool reaches every agent's tool list now.
            crate::agents::push_profiles(app);
            Response::json(200, &json!({ "state": state }))
        }
        Err(e) => Response::error(409, &e),
    }
}

fn delete(app: &App, name: &str, version: &str) -> Response {
    let Ok(version) = version.parse::<u32>() else {
        return Response::error(400, "the version must be a number");
    };
    match app.library.delete(name, version) {
        Ok(()) => Response::json(200, &json!({})),
        Err(e) => Response::error(404, &e),
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "tool name {name:?} may use letters, digits, '_' and '-' only"
        ))
    }
}

fn check_rel_path(path: &str) -> Result<(), String> {
    let ok = !path.is_empty()
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "file path {path:?} must be relative and stay inside the package"
        ))
    }
}

/// The content digest the guest checks before running a program: SHA-256 over the files
/// sorted by path, each as path, a NUL, its length (8 bytes, big-endian) and its bytes —
/// exactly as the harness computes it (`agent-harness/src/library.rs`).
fn digest(blobs: &[(String, Vec<u8>)]) -> String {
    use sha2::{Digest, Sha256};
    let mut sorted: Vec<&(String, Vec<u8>)> = blobs.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = Sha256::new();
    for (path, bytes) in sorted {
        hasher.update(path.as_bytes());
        hasher.update([0u8]);
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn base64_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for c in text
        .bytes()
        .filter(|c| !c.is_ascii_whitespace() && *c != b'=')
    {
        let value = B64.iter().position(|x| *x == c)? as u32;
        acc = (acc << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

impl Store {
    /// The entries as they are now (the hub's saver writes what changed).
    pub fn rows(&self) -> Result<apz_store::RowSet, apz_store::StoreError> {
        let entries = self.lock();
        apz_store::RowSet::list(def(), entries.iter())
    }

    /// Replaces the entries with what the hub's store holds (at start), in their stored order.
    pub fn load(
        &self,
        conn: &apz_store::rusqlite::Connection,
    ) -> Result<(), apz_store::StoreError> {
        let stored: Vec<Entry> = apz_store::table::load_list(conn, def())?;
        *self.lock() = stored;
        Ok(())
    }
}

/// The library's tables: `library_entry`, with its files and its command and requires lists as child tables.
pub fn def() -> &'static apz_store::TableDef {
    apz_store::root_def!(Entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entries_come_back_whole_from_their_table() {
        let a = store("roundtrip-a");
        let mut tool = package("tool", true);
        tool.requires = vec!["requests".to_string()];
        tool.input_schema =
            json!({ "type": "object", "properties": { "n": { "type": "integer" } } });
        tool.files.push(PackageFile {
            path: "lib/util.py".to_string(),
            content_b64: base64_encode(b"def twice(n): return 2 * n"),
            executable: true,
        });
        a.insert(1, "alpha", tool).unwrap();
        a.review("tool", 1, true, "fine").unwrap();
        a.insert(2, "beta", package("tool", false)).unwrap();
        assert!(a.program(1, "x_tool").is_some());
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &a.rows().unwrap()).unwrap();
        let b = store("roundtrip-b");
        b.load(&conn).unwrap();
        assert_eq!(b.all().len(), 2);
        assert_eq!(
            serde_json::to_value(&*b.lock()).unwrap(),
            serde_json::to_value(&*a.lock()).unwrap()
        );
    }

    fn store(tag: &str) -> Store {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        Store::open(std::env::temp_dir().join(format!("apid-library-{tag}-{nanos}")))
    }

    fn package(name: &str, publish: bool) -> Package {
        Package {
            name: name.to_string(),
            description: "adds numbers".to_string(),
            command: vec!["python3".to_string(), "main.py".to_string()],
            requires: Vec::new(),
            input_schema: json!({ "type": "object" }),
            publish,
            files: vec![PackageFile {
                path: "main.py".to_string(),
                content_b64: base64_encode(b"print(1)"),
                executable: false,
            }],
        }
    }

    #[test]
    fn accept_stores_a_private_version_with_its_files() {
        let store = store("accept");
        let entry = store.insert(7, "scout", package("adder", false)).unwrap();
        assert_eq!((entry.version, entry.state.as_str()), (1, "private"));
        let on_disk = fs::read(store.files_dir("adder", 1).join("main.py")).unwrap();
        assert_eq!(on_disk, b"print(1)");
    }

    #[test]
    fn publish_waits_for_review_and_review_publishes() {
        let store = store("review");
        let entry = store.insert(7, "scout", package("adder", true)).unwrap();
        assert_eq!(entry.state, "pending");
        assert_eq!(store.review("adder", 1, true, "fine").unwrap(), "published");
        assert!(store.review("adder", 1, true, "again").is_err());
    }

    #[test]
    fn others_see_only_published_versions_and_the_author_sees_its_own_draft() {
        let store = store("visibility");
        store.insert(7, "scout", package("adder", true)).unwrap();
        assert!(store.offered(8).is_empty());
        store.review("adder", 1, true, "").unwrap();
        assert_eq!(store.offered(8)[0].version, 1);
        store.insert(7, "scout", package("adder", false)).unwrap();
        assert_eq!(store.offered(7)[0].version, 2);
        assert_eq!(store.offered(8)[0].version, 1);
    }

    #[test]
    fn program_assembles_the_visible_files_and_counts_the_call() {
        let store = store("program");
        store.insert(7, "scout", package("adder", true)).unwrap();
        store.review("adder", 1, true, "").unwrap();
        assert!(store.program(8, "x_adder").is_some());
        assert!(store.program(8, "x_missing").is_none());
        assert!(store.program(8, "adder").is_none());
        let program = store.program(7, "x_adder").unwrap();
        assert_eq!(
            (program.name.as_str(), program.version, program.timeout_s),
            ("adder", 1, 120)
        );
        assert_eq!(
            base64_decode(&program.files[0].content_b64).unwrap(),
            b"print(1)"
        );
        assert_eq!(store.all()[0].calls, 2);
    }
}
