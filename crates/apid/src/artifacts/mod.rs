//! Artifacts (docs/design/artifacts.md): what agents show of their work — charts, tables, graphs, dashboards,
//! images, datasets, documents, code and live HTML — as `apz_view` panels, versioned, with their data blobs in the
//! deliverables store (Garage or the hub's disk).
//!
//! The index (each artifact's versions, its author and its blob references, and each blob's owner) is held in
//! memory and saved atomically to `<state-dir>/artifacts.json`. Each version's spec is an object at
//! `artifacts/<id>/v<n>.json` and each blob an object at `artifacts/blobs/<blob id>`, both in the deliverables
//! store's objects.

pub mod gateway;
pub mod routes;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use apz_view::artifact::{Artifact, Caps, Spec, Summary, kind_of, libs_used, validate};
use apz_view::{BlobRef, Body, ColumnKind, DatasetColumn, Panel};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::deliverables;
use crate::providers::write_atomic;

/// The index file under the state directory.
const INDEX: &str = "artifacts.json";
/// The most rows one [`Store::rows`] call answers.
const MAX_PAGE: u64 = 500;
/// The page size when a caller gives no limit.
const DEFAULT_PAGE: u64 = 100;
/// The alphabet of artifact ids: RFC 4648 base32, lower case.
const BASE32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
/// How many parsed dataset blobs the store keeps, the most recent first.
const PARSED_BLOBS: usize = 4;
/// The largest dataset blob that is parsed and kept for paging. A larger one is refused for paging (an export still
/// reads it, once).
const PAGED_BYTES: u64 = 64 << 20;
/// The longest content type a blob may carry, parameters included.
const MAX_CONTENT_TYPE: usize = 100;

/// One version of an artifact as `GET /v1/artifacts/{id}/versions` lists it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: u32,
    pub created_ms: u64,
    pub title: String,
}

/// One page of a dataset's rows. `total` counts the rows after the filter, before the page.
#[derive(Clone, Debug, Serialize)]
pub struct RowsPage {
    pub columns: Vec<DatasetColumn>,
    pub rows: Vec<Vec<Value>>,
    pub total: u64,
    pub offset: u64,
}

/// Why a write was refused: the agent's daily budget is spent, or the request does not check out.
#[derive(Debug)]
pub enum Refusal {
    /// The budget's own message, when the budget is spent.
    Budget(String),
    /// The problems the agent can act on.
    Problems(Vec<String>),
}

impl Refusal {
    /// The refusal as a list of messages, the budget's message included.
    #[cfg(test)]
    fn into_problems(self) -> Vec<String> {
        match self {
            Refusal::Budget(why) => vec![why],
            Refusal::Problems(problems) => problems,
        }
    }
}

/// The daily budget a write is checked against: the start of the window, and the check of the window's totals,
/// which answers the refusal, or `None` while there is room.
pub struct Budget {
    pub since_ms: u64,
    pub check: fn(u32, u64) -> Option<String>,
}

impl Budget {
    /// A budget that refuses nothing, for the owner's writes and for tests.
    #[cfg(test)]
    pub fn unlimited() -> Budget {
        Budget {
            since_ms: 0,
            check: no_limit,
        }
    }
}

#[cfg(test)]
fn no_limit(_: u32, _: u64) -> Option<String> {
    None
}

/// The persisted index: every artifact and every blob.
#[derive(Clone, Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    artifacts: Vec<Entry>,
    #[serde(default)]
    blobs: BTreeMap<String, Blob>,
}

/// One artifact in the index: who made it and each of its versions.
#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    id: String,
    /// The agent that made it; `None` for the owner.
    author_cid: Option<u64>,
    author: String,
    project: Option<String>,
    created_ms: u64,
    updated_ms: u64,
    shared: bool,
    /// Oldest first; the last is the newest.
    versions: Vec<Version>,
}

/// One version of an artifact in the index.
#[derive(Clone, Serialize, Deserialize)]
struct Version {
    version: u32,
    created_ms: u64,
    title: String,
    kind: String,
    /// The spec's JSON bytes.
    spec_bytes: u64,
    /// The spec's bytes and the bytes of the blobs it references.
    bytes: u64,
    /// The ids of the blobs it references.
    blobs: Vec<String>,
}

/// One blob in the index: whose it is, and what it holds.
#[derive(Clone, Serialize, Deserialize)]
struct Blob {
    /// The agent that attached it; `None` for the owner.
    cid: Option<u64>,
    content_type: String,
    bytes: u64,
    created_ms: u64,
}

/// A blob's rows, parsed once: CSV records (the first is the header row) as text fields, or JSON Lines values.
enum Parsed {
    Csv(Vec<Vec<String>>),
    Json(Vec<Value>),
}

/// The artifact store.
pub struct Store {
    /// `<state-dir>/artifacts.json`.
    path: PathBuf,
    /// Where the version specs and the blobs are kept.
    objects: Arc<deliverables::Store>,
    index: Mutex<Index>,
    /// The parsed rows of recent dataset blobs, the most recent first (at most [`PARSED_BLOBS`]).
    recent: Mutex<Vec<(String, Arc<Parsed>)>>,
}

impl Store {
    /// Loads `<state-dir>/artifacts.json` (a missing file is an empty store). An index that cannot be read is set
    /// aside as `artifacts.json.corrupt-<unix ms>` and logged, never overwritten.
    pub fn load(state_dir: &Path, objects: Arc<deliverables::Store>) -> Store {
        let _ = std::fs::create_dir_all(state_dir);
        let path = state_dir.join(INDEX);
        let index = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Index>(&bytes) {
                Ok(index) => index,
                Err(e) => {
                    set_aside(&path, &e);
                    Index::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Index::default(),
            Err(e) => {
                set_aside(&path, &e);
                Index::default()
            }
        };
        Store {
            path,
            objects,
            index: Mutex::new(index),
            recent: Mutex::new(Vec::new()),
        }
    }

    /// A fresh artifact id that no artifact uses yet. The caller passes it to [`Store::create_within`].
    pub fn reserve_id(&self) -> String {
        new_id(&lock(&self.index))
    }

    /// Makes a new artifact `id` at version 1, checked against `budget` under the store's lock, so that the check and
    /// the write cannot interleave with another write. Refused with the problems when the spec does not validate or
    /// names a blob that is missing or belongs to another agent. `cid` is `None` for the owner.
    #[allow(clippy::too_many_arguments)]
    pub fn create_within(
        &self,
        id: &str,
        author: &str,
        cid: Option<u64>,
        project: Option<String>,
        spec: Spec,
        budget: &Budget,
    ) -> Result<Artifact, Refusal> {
        let mut index = lock(&self.index);
        let (count, bytes) = stats_of(&index, cid, budget.since_ms);
        if let Some(why) = (budget.check)(count, bytes) {
            return Err(Refusal::Budget(why));
        }
        if index.artifacts.iter().any(|e| e.id == id) {
            return Err(Refusal::Problems(vec![format!(
                "artifact {id:?} already exists"
            )]));
        }
        let mut spec = spec;
        let now = now_ms();
        let (json, meta) = prepare(&mut spec, 1, cid, &index, now).map_err(Refusal::Problems)?;
        let scripts = compile_scripts(&spec).map_err(Refusal::Problems)?;
        self.objects
            .put_object(&spec_key(id, 1), &json)
            .map_err(|e| Refusal::Problems(vec![format!("cannot store the spec: {e}")]))?;
        self.store_scripts(id, 1, &scripts)
            .map_err(Refusal::Problems)?;
        let entry = Entry {
            id: id.to_string(),
            author_cid: cid,
            author: author.to_string(),
            project,
            created_ms: now,
            updated_ms: now,
            shared: false,
            versions: vec![meta.clone()],
        };
        let artifact = assemble(&entry, 1, &meta, spec);
        let mut next = (*index).clone();
        next.artifacts.push(entry);
        self.save(&next).map_err(|e| Refusal::Problems(vec![e]))?;
        *index = next;
        Ok(artifact)
    }

    /// Makes a new artifact at version 1 with a fresh id and no budget (the owner's writes, and tests). See
    /// [`Store::create_within`].
    #[cfg(test)]
    pub fn create(
        &self,
        author: &str,
        cid: Option<u64>,
        project: Option<String>,
        spec: Spec,
    ) -> Result<Artifact, Vec<String>> {
        let id = self.reserve_id();
        self.create_within(&id, author, cid, project, spec, &Budget::unlimited())
            .map_err(Refusal::into_problems)
    }

    /// Adds version `n + 1` of artifact `id`, checked against `budget` under the store's lock. Only its author (or the
    /// owner, `cid` `None`) may update it.
    pub fn update_within(
        &self,
        id: &str,
        cid: Option<u64>,
        spec: Spec,
        budget: &Budget,
    ) -> Result<Artifact, Refusal> {
        let mut index = lock(&self.index);
        let pos = index
            .artifacts
            .iter()
            .position(|e| e.id == id)
            .ok_or_else(|| Refusal::Problems(vec![format!("no artifact {id:?}")]))?;
        let entry = &index.artifacts[pos];
        if cid.is_some() && entry.author_cid != cid {
            return Err(Refusal::Problems(vec![
                "only the artifact's author can update it".to_string(),
            ]));
        }
        let version = entry.versions.last().map_or(1, |v| v.version + 1);
        let (count, bytes) = stats_of(&index, cid, budget.since_ms);
        if let Some(why) = (budget.check)(count, bytes) {
            return Err(Refusal::Budget(why));
        }
        let mut spec = spec;
        let now = now_ms();
        let (json, meta) =
            prepare(&mut spec, version, cid, &index, now).map_err(Refusal::Problems)?;
        let scripts = compile_scripts(&spec).map_err(Refusal::Problems)?;
        self.objects
            .put_object(&spec_key(id, version), &json)
            .map_err(|e| Refusal::Problems(vec![format!("cannot store the spec: {e}")]))?;
        self.store_scripts(id, version, &scripts)
            .map_err(Refusal::Problems)?;
        let mut next = (*index).clone();
        next.artifacts[pos].versions.push(meta.clone());
        next.artifacts[pos].updated_ms = now;
        let artifact = assemble(&next.artifacts[pos], version, &meta, spec);
        self.save(&next).map_err(|e| Refusal::Problems(vec![e]))?;
        *index = next;
        Ok(artifact)
    }

    /// Adds a version with no budget (the owner's writes, and tests). See [`Store::update_within`].
    #[cfg(test)]
    pub fn update(&self, id: &str, cid: Option<u64>, spec: Spec) -> Result<Artifact, Vec<String>> {
        self.update_within(id, cid, spec, &Budget::unlimited())
            .map_err(Refusal::into_problems)
    }

    /// Artifact `id` at `version` (its newest when `None`), with its spec.
    pub fn get(&self, id: &str, version: Option<u32>) -> Option<Artifact> {
        let entry = lock(&self.index)
            .artifacts
            .iter()
            .find(|e| e.id == id)
            .cloned()?;
        let latest = entry.versions.last()?.version;
        let wanted = version.unwrap_or(latest);
        let meta = entry.versions.iter().find(|v| v.version == wanted)?;
        let bytes = self.objects.get_object(&spec_key(id, wanted)).ok()?;
        let spec: Spec = serde_json::from_slice(&bytes).ok()?;
        Some(assemble(&entry, latest, meta, spec))
    }

    /// Whether artifact `id` exists.
    pub fn exists(&self, id: &str) -> bool {
        lock(&self.index).artifacts.iter().any(|e| e.id == id)
    }

    /// The artifacts, newest first, optionally only those of `project` and those whose title or author contains
    /// `query` (case-insensitive).
    pub fn list(&self, project: Option<&str>, query: Option<&str>) -> Vec<Summary> {
        let needle = query
            .map(str::to_lowercase)
            .filter(|q| !q.trim().is_empty());
        let index = lock(&self.index);
        let mut out: Vec<Summary> = index
            .artifacts
            .iter()
            .rev()
            .filter_map(|e| {
                let latest = e.versions.last()?;
                if let Some(p) = project
                    && e.project.as_deref() != Some(p)
                {
                    return None;
                }
                if let Some(q) = needle.as_deref()
                    && !latest.title.to_lowercase().contains(q)
                    && !e.author.to_lowercase().contains(q)
                {
                    return None;
                }
                Some(Summary {
                    id: e.id.clone(),
                    title: latest.title.clone(),
                    kind: latest.kind.clone(),
                    latest: latest.version,
                    author: e.author.clone(),
                    project: e.project.clone(),
                    updated_ms: e.updated_ms,
                    bytes: latest.bytes,
                    shared: e.shared,
                })
            })
            .collect();
        out.sort_by_key(|a| std::cmp::Reverse(a.updated_ms));
        out
    }

    /// The versions of artifact `id`, oldest first (empty when it does not exist).
    pub fn versions(&self, id: &str) -> Vec<VersionInfo> {
        let index = lock(&self.index);
        index
            .artifacts
            .iter()
            .find(|e| e.id == id)
            .map_or_else(Vec::new, |e| {
                e.versions
                    .iter()
                    .map(|v| VersionInfo {
                        version: v.version,
                        created_ms: v.created_ms,
                        title: v.title.clone(),
                    })
                    .collect()
            })
    }

    /// Deletes artifact `id`, its specs, and the blobs no other artifact still uses. False when it did not exist.
    pub fn delete(&self, id: &str) -> bool {
        // The compiled scripts of every version go with the artifact; they are listed before the lock is taken.
        let mut stale: Vec<String> = vec![];
        for v in self.versions(id) {
            if let Some(artifact) = self.get(id, Some(v.version)) {
                stale.extend(
                    script_files(&artifact.panels)
                        .into_iter()
                        .map(|(panel, path, _)| compiled_key(id, v.version, panel, path)),
                );
            }
        }
        let mut index = lock(&self.index);
        let Some(pos) = index.artifacts.iter().position(|e| e.id == id) else {
            return false;
        };
        let mut next = (*index).clone();
        let gone = next.artifacts.remove(pos);
        let mut orphans: Vec<String> = vec![];
        for blob in gone.versions.iter().flat_map(|v| v.blobs.iter()) {
            let still_used = next
                .artifacts
                .iter()
                .any(|e| e.versions.iter().any(|v| v.blobs.contains(blob)));
            if !still_used && !orphans.contains(blob) {
                orphans.push(blob.clone());
            }
        }
        for blob in &orphans {
            next.blobs.remove(blob);
        }
        if self.save(&next).is_err() {
            return false;
        }
        *index = next;
        // The objects go after the index: a leftover object is harmless, a missing one would break a live artifact.
        for v in &gone.versions {
            let _ = self.objects.delete_object(&spec_key(id, v.version));
        }
        for blob in &orphans {
            let _ = self.objects.delete_object(&blob_key(blob));
        }
        for key in &stale {
            let _ = self.objects.delete_object(key);
        }
        true
    }

    /// The agent that made artifact `id`; `None` for the owner or when it does not exist.
    pub fn author_cid(&self, id: &str) -> Option<u64> {
        lock(&self.index)
            .artifacts
            .iter()
            .find(|e| e.id == id)
            .and_then(|e| e.author_cid)
    }

    /// Stores a data blob (CSV, JSON Lines, an image, an HTML file) that a spec can then reference by its id, checked
    /// against `budget` under the store's lock. `cid` is the agent that attaches it; `None` for the owner. The content
    /// type is kept only in the form [`safe_content_type`] allows.
    pub fn put_blob_within(
        &self,
        cid: Option<u64>,
        content_type: &str,
        bytes: &[u8],
        budget: &Budget,
    ) -> Result<BlobRef, Refusal> {
        if bytes.is_empty() {
            return Err(Refusal::Problems(vec!["the data is empty".to_string()]));
        }
        let limit = Caps::default().blob_bytes;
        if bytes.len() as u64 > limit {
            return Err(Refusal::Problems(vec![format!(
                "one data file is at most {} MiB",
                limit >> 20
            )]));
        }
        let content_type = safe_content_type(content_type);
        let mut index = lock(&self.index);
        let (count, total) = stats_of(&index, cid, budget.since_ms);
        if let Some(why) = (budget.check)(count, total) {
            return Err(Refusal::Budget(why));
        }
        let id = format!("b{}", hex(&random_bytes(8)));
        self.objects
            .put_object(&blob_key(&id), bytes)
            .map_err(|e| Refusal::Problems(vec![e.to_string()]))?;
        let size = bytes.len() as u64;
        let mut next = (*index).clone();
        next.blobs.insert(
            id.clone(),
            Blob {
                cid,
                content_type: content_type.clone(),
                bytes: size,
                created_ms: now_ms(),
            },
        );
        self.save(&next).map_err(|e| Refusal::Problems(vec![e]))?;
        *index = next;
        Ok(BlobRef {
            id,
            content_type,
            bytes: size,
        })
    }

    /// Stores a data blob with no budget (the owner's writes, and tests). See [`Store::put_blob_within`].
    #[cfg(test)]
    pub fn put_blob(
        &self,
        cid: Option<u64>,
        content_type: &str,
        bytes: &[u8],
    ) -> Result<BlobRef, String> {
        self.put_blob_within(cid, content_type, bytes, &Budget::unlimited())
            .map_err(|refusal| refusal.into_problems().join("; "))
    }

    /// The content type and bytes of blob `blob_id`.
    pub fn blob(&self, blob_id: &str) -> Option<(String, Vec<u8>)> {
        let content_type = lock(&self.index).blobs.get(blob_id)?.content_type.clone();
        let bytes = self.objects.get_object(&blob_key(blob_id)).ok()?;
        Some((content_type, bytes))
    }

    /// Blob `blob_id`, when some version of artifact `id` references it.
    pub fn artifact_blob(&self, id: &str, blob_id: &str) -> Option<(String, Vec<u8>)> {
        let referenced = lock(&self.index)
            .artifacts
            .iter()
            .find(|e| e.id == id)
            .is_some_and(|e| {
                e.versions
                    .iter()
                    .any(|v| v.blobs.iter().any(|b| b == blob_id))
            });
        if !referenced {
            return None;
        }
        self.blob(blob_id)
    }

    /// The compiled JavaScript of script `path` of panel `panel` at `version` of artifact `id`, or `None` when that
    /// version has no such compiled script (the path is not a script of that panel, or the version does not exist).
    pub fn compiled(&self, id: &str, version: u32, panel: &str, path: &str) -> Option<String> {
        let bytes = self
            .objects
            .get_object(&compiled_key(id, version, panel, path))
            .ok()?;
        String::from_utf8(bytes).ok()
    }

    /// Stores the compiled scripts of version `version` of artifact `id`, as `(panel, path, JavaScript)` triples.
    fn store_scripts(
        &self,
        id: &str,
        version: u32,
        scripts: &[(String, String, String)],
    ) -> Result<(), Vec<String>> {
        for (panel, path, js) in scripts {
            self.objects
                .put_object(&compiled_key(id, version, panel, path), js.as_bytes())
                .map_err(|e| vec![format!("cannot store the compiled script {path}: {e}")])?;
        }
        Ok(())
    }

    /// One page of the rows of dataset panel `panel` at `version` (the newest when `None`): filtered (a
    /// case-insensitive substring over every cell), sorted by column `sort` (`(key, descending)`; numeric when every
    /// value in the column is a number), then cut to `limit` rows (500 at most, 100 when 0) from `offset`. A dataset
    /// blob larger than 64 MiB is refused for paging.
    #[allow(clippy::too_many_arguments)]
    pub fn rows(
        &self,
        id: &str,
        version: Option<u32>,
        panel: &str,
        offset: u64,
        limit: u64,
        sort: Option<(&str, bool)>,
        filter: Option<&str>,
    ) -> Result<RowsPage, String> {
        let (columns, mut rows) = self.dataset_rows(id, version, panel, PAGED_BYTES)?;
        if let Some(needle) = filter.map(str::to_lowercase).filter(|n| !n.is_empty()) {
            rows.retain(|row| {
                row.iter()
                    .any(|cell| value_text(cell).to_lowercase().contains(&needle))
            });
        }
        if let Some((key, desc)) = sort
            && let Some(i) = columns.iter().position(|c| c.key == key)
        {
            let numeric = rows.iter().all(|r| {
                r.get(i)
                    .is_none_or(|v| v.is_null() || as_number(v).is_some())
            });
            let null = Value::Null;
            rows.sort_by(|a, b| {
                let x = a.get(i).unwrap_or(&null);
                let y = b.get(i).unwrap_or(&null);
                let ord = if numeric {
                    as_number(x)
                        .unwrap_or(f64::NEG_INFINITY)
                        .total_cmp(&as_number(y).unwrap_or(f64::NEG_INFINITY))
                } else {
                    value_text(x).cmp(&value_text(y))
                };
                if desc { ord.reverse() } else { ord }
            });
        }
        let total = rows.len() as u64;
        let limit = if limit == 0 {
            DEFAULT_PAGE
        } else {
            limit.min(MAX_PAGE)
        };
        let page: Vec<Vec<Value>> = rows
            .into_iter()
            .skip(offset.min(total) as usize)
            .take(limit as usize)
            .collect();
        Ok(RowsPage {
            columns,
            rows: page,
            total,
            offset,
        })
    }

    /// Every row of dataset panel `panel` at `version` (the newest when `None`), with its columns. Inline rows are
    /// returned as they are; a blob is read as CSV with a header row, or as JSON Lines (arrays or objects keyed by
    /// column key). A blob larger than `max_bytes` is refused.
    pub fn dataset_rows(
        &self,
        id: &str,
        version: Option<u32>,
        panel: &str,
        max_bytes: u64,
    ) -> Result<(Vec<DatasetColumn>, Vec<Vec<Value>>), String> {
        let artifact = self
            .get(id, version)
            .ok_or_else(|| "no such artifact or version".to_string())?;
        let found = artifact
            .panels
            .iter()
            .find(|p| p.id == panel)
            .ok_or_else(|| format!("no panel {panel:?}"))?;
        let Body::Dataset(d) = &found.body else {
            return Err(format!("panel {panel:?} is not a dataset"));
        };
        let rows = match &d.blob {
            None => d.rows.clone(),
            Some(blob) => match self.parsed_blob(&blob.id, max_bytes)?.as_ref() {
                Parsed::Csv(records) => csv_rows(records, &d.columns),
                Parsed::Json(values) => json_rows(values, &d.columns),
            },
        };
        Ok((d.columns.clone(), rows))
    }

    /// The parsed rows of blob `blob_id`: from the recent ones when they are there, else read, parsed and kept.
    fn parsed_blob(&self, blob_id: &str, max_bytes: u64) -> Result<Arc<Parsed>, String> {
        if let Some(hit) = self.recent_parsed(blob_id) {
            return Ok(hit);
        }
        let size = lock(&self.index)
            .blobs
            .get(blob_id)
            .map(|b| b.bytes)
            .ok_or_else(|| format!("the rows blob {blob_id:?} is missing"))?;
        if size > max_bytes {
            return Err(format!(
                "the rows blob is {} MiB; paging takes datasets of at most {} MiB: filter or export it instead",
                size >> 20,
                max_bytes >> 20
            ));
        }
        let (content_type, bytes) = self
            .blob(blob_id)
            .ok_or_else(|| format!("the rows blob {blob_id:?} is missing"))?;
        let text =
            std::str::from_utf8(&bytes).map_err(|_| "the rows are not UTF-8 text".to_string())?;
        let parsed = Arc::new(if is_json_lines(&content_type, text) {
            Parsed::Json(parse_json_lines(text)?)
        } else {
            Parsed::Csv(parse_csv(text))
        });
        if size <= PAGED_BYTES {
            self.remember_parsed(blob_id, parsed.clone());
        }
        Ok(parsed)
    }

    /// The parsed rows of blob `blob_id` when they are among the recent ones, which move to the front as they are used.
    fn recent_parsed(&self, blob_id: &str) -> Option<Arc<Parsed>> {
        let mut recent = lock(&self.recent);
        let pos = recent.iter().position(|(id, _)| id == blob_id)?;
        let entry = recent.remove(pos);
        let parsed = entry.1.clone();
        recent.insert(0, entry);
        Some(parsed)
    }

    /// Keeps the parsed rows of blob `blob_id` as the most recent, dropping the oldest beyond [`PARSED_BLOBS`].
    fn remember_parsed(&self, blob_id: &str, parsed: Arc<Parsed>) {
        let mut recent = lock(&self.recent);
        recent.retain(|(id, _)| id != blob_id);
        recent.insert(0, (blob_id.to_string(), parsed));
        recent.truncate(PARSED_BLOBS);
    }

    /// Marks artifact `id` shared (or not). A save that fails leaves the flag as it was.
    pub fn set_shared(&self, id: &str, shared: bool) {
        let mut index = lock(&self.index);
        let Some(pos) = index.artifacts.iter().position(|e| e.id == id) else {
            return;
        };
        if index.artifacts[pos].shared == shared {
            return;
        }
        let mut next = (*index).clone();
        next.artifacts[pos].shared = shared;
        if self.save(&next).is_ok() {
            *index = next;
        }
    }

    /// Saves `index` to `artifacts.json`, atomically.
    fn save(&self, index: &Index) -> Result<(), String> {
        let bytes =
            serde_json::to_vec(index).map_err(|e| format!("cannot serialize the index: {e}"))?;
        write_atomic(&self.path, &bytes, 0o600).map_err(|e| format!("cannot save the index: {e}"))
    }
}

/// The writes that agent `cid` (`None` for the owner) made since `since_ms`, as [`Store::stats`] counts them.
fn stats_of(index: &Index, cid: Option<u64>, since_ms: u64) -> (u32, u64) {
    let mut count = 0u32;
    let mut bytes = 0u64;
    for e in index.artifacts.iter().filter(|e| e.author_cid == cid) {
        for v in e.versions.iter().filter(|v| v.created_ms >= since_ms) {
            count = count.saturating_add(1);
            bytes += v.spec_bytes;
        }
    }
    for b in index
        .blobs
        .values()
        .filter(|b| b.cid == cid && b.created_ms >= since_ms)
    {
        bytes += b.bytes;
    }
    (count, bytes)
}

/// Sets an index that cannot be read aside as `artifacts.json.corrupt-<unix ms>` and logs why, so that the store
/// starts empty without overwriting what was there.
fn set_aside(path: &Path, why: &dyn std::fmt::Display) {
    let aside = path.with_file_name(format!("{INDEX}.corrupt-{}", now_ms()));
    eprintln!(
        "artifacts: cannot read {}: {why}; keeping it at {}",
        path.display(),
        aside.display()
    );
    if let Err(e) = std::fs::rename(path, &aside) {
        eprintln!("artifacts: cannot set {} aside: {e}", path.display());
    }
}

/// A blob's content type as it may be served: `type/subtype` of token characters, with `; charset=utf-8` allowed after
/// it and nothing else. Anything else, including an empty type or one with a line break, becomes
/// `application/octet-stream`, so that no header value can carry a control character.
pub fn safe_content_type(raw: &str) -> String {
    const FALLBACK: &str = "application/octet-stream";
    let raw = raw.trim();
    let (media, params) = match raw.split_once(';') {
        Some((media, params)) => (media.trim_end(), Some(params.trim())),
        None => (raw, None),
    };
    let token = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&c))
    };
    let well_formed = media
        .split_once('/')
        .is_some_and(|(kind, sub)| token(kind) && token(sub));
    let params_ok = params.is_none_or(|p| p.eq_ignore_ascii_case("charset=utf-8"));
    if !(well_formed && params_ok && raw.len() <= MAX_CONTENT_TYPE) {
        return FALLBACK.to_string();
    }
    match params {
        Some(_) => format!("{media}; charset=utf-8"),
        None => media.to_string(),
    }
}

/// Checks a spec for a new version `version` by agent `cid` (`None` for the owner) and builds the version's record
/// and its spec's JSON. Fills each HTML body's libraries from its files first.
fn prepare(
    spec: &mut Spec,
    version: u32,
    cid: Option<u64>,
    index: &Index,
    now: u64,
) -> Result<(Vec<u8>, Version), Vec<String>> {
    for panel in &mut spec.panels {
        if let Body::Html(h) = &mut panel.body {
            let libs: Vec<String> = libs_used(h).into_iter().map(str::to_string).collect();
            h.libs = libs;
        }
    }
    let mut problems = validate(spec, &Caps::default());
    let mut blobs: Vec<String> = vec![];
    let mut blob_bytes = 0u64;
    for r in blob_refs(spec) {
        if r.id.is_empty() {
            continue;
        }
        match index.blobs.get(&r.id) {
            None => problems.push(format!(
                "blob {:?} does not exist: attach the data with artifact_attach_data first",
                r.id
            )),
            Some(b) => {
                if cid.is_some() && b.cid != cid {
                    problems.push(format!("blob {:?} is not yours", r.id));
                }
                if !blobs.contains(&r.id) {
                    blob_bytes += b.bytes;
                    blobs.push(r.id.clone());
                }
            }
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let json =
        serde_json::to_vec(&*spec).map_err(|e| vec![format!("cannot serialize the spec: {e}")])?;
    let spec_bytes = json.len() as u64;
    let meta = Version {
        version,
        created_ms: now,
        title: spec.title.clone(),
        kind: kind_of(&spec.panels).to_string(),
        spec_bytes,
        bytes: spec_bytes + blob_bytes,
        blobs,
    };
    Ok((json, meta))
}

/// The blobs a spec references: image bodies, dataset rows, HTML files and stills.
fn blob_refs(spec: &Spec) -> Vec<&BlobRef> {
    let mut out = vec![];
    for panel in &spec.panels {
        match &panel.body {
            Body::Image(img) => out.push(&img.blob),
            Body::Dataset(d) => out.extend(d.blob.as_ref()),
            Body::Html(h) => {
                for f in &h.files {
                    out.extend(f.blob.as_ref());
                }
                out.extend(h.still.as_ref());
            }
            _ => {}
        }
    }
    out
}

/// The artifact `entry` at `latest`, showing `spec` as version `version`.
fn assemble(entry: &Entry, latest: u32, meta: &Version, spec: Spec) -> Artifact {
    Artifact {
        id: entry.id.clone(),
        version: meta.version,
        latest,
        title: spec.title,
        sub: spec.sub,
        panels: spec.panels,
        kind: meta.kind.clone(),
        author: entry.author.clone(),
        project: entry.project.clone(),
        created_ms: entry.created_ms,
        updated_ms: entry.updated_ms,
        bytes: meta.bytes,
    }
}

/// The rows of a dataset from parsed CSV records, the first of which is the header row: the column for each dataset
/// column is the header named by its key, else its position.
fn csv_rows(records: &[Vec<String>], columns: &[DatasetColumn]) -> Vec<Vec<Value>> {
    let header: &[String] = records.first().map(Vec::as_slice).unwrap_or(&[]);
    let positions: Vec<usize> = columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            header
                .iter()
                .position(|h| h.trim() == c.key.as_str())
                .unwrap_or(i)
        })
        .collect();
    records
        .iter()
        .skip(1)
        .map(|record| {
            columns
                .iter()
                .zip(&positions)
                .map(|(c, &p)| cell_value(&c.kind, record.get(p).map_or("", String::as_str)))
                .collect()
        })
        .collect()
}

/// Parses JSON Lines: each non-empty line must be a JSON array (values in column order) or object (keyed by column
/// key). The error names the first line that is not.
fn parse_json_lines(text: &str) -> Result<Vec<Value>, String> {
    let mut values = vec![];
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value =
            serde_json::from_str(line).map_err(|e| format!("line {}: {e}", n + 1))?;
        if !matches!(value, Value::Array(_) | Value::Object(_)) {
            return Err(format!(
                "line {} is neither a JSON array nor a JSON object",
                n + 1
            ));
        }
        values.push(value);
    }
    Ok(values)
}

/// The rows of a dataset from parsed JSON Lines values, in the dataset's column order.
fn json_rows(values: &[Value], columns: &[DatasetColumn]) -> Vec<Vec<Value>> {
    values
        .iter()
        .map(|value| match value {
            Value::Array(items) => (0..columns.len())
                .map(|i| items.get(i).cloned().unwrap_or(Value::Null))
                .collect(),
            Value::Object(map) => columns
                .iter()
                .map(|c| map.get(&c.key).cloned().unwrap_or(Value::Null))
                .collect(),
            _ => vec![Value::Null; columns.len()],
        })
        .collect()
}

/// Whether a rows blob is JSON Lines: a JSON content type, or text that starts like JSON.
fn is_json_lines(content_type: &str, text: &str) -> bool {
    content_type.contains("json") || matches!(text.trim_start().chars().next(), Some('{' | '['))
}

/// A CSV field as a cell: empty is null, a number column's numbers are numbers, the rest are text.
fn cell_value(kind: &ColumnKind, text: &str) -> Value {
    if text.is_empty() {
        return Value::Null;
    }
    if matches!(kind, ColumnKind::Number)
        && let Some(n) = text
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
    {
        return Value::Number(n);
    }
    Value::String(text.to_string())
}

/// Splits CSV into records of fields: comma-separated, double quotes around fields (a doubled quote is one quote,
/// and a quoted field may hold commas and line breaks), CRLF or LF line ends, blank lines skipped.
fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut records: Vec<Vec<String>> = vec![];
    let mut record: Vec<String> = vec![];
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            ',' => record.push(std::mem::take(&mut field)),
            '\r' => {}
            '\n' => {
                record.push(std::mem::take(&mut field));
                if record.len() == 1 && record[0].is_empty() {
                    record.clear();
                } else {
                    records.push(std::mem::take(&mut record));
                }
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    records
}

/// A cell's text for filtering and CSV: null is empty, a string is itself, anything else its JSON.
pub(crate) fn value_text(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// A cell as a number, when it is one (a number, or text that parses as one).
fn as_number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// `a` and eleven base32 characters from the OS random source, not yet used by an artifact of `index`.
fn new_id(index: &Index) -> String {
    loop {
        let bytes = random_bytes(11);
        let id: String = std::iter::once('a')
            .chain(bytes.iter().map(|b| BASE32[(b % 32) as usize] as char))
            .collect();
        if !index.artifacts.iter().any(|e| e.id == id) {
            return id;
        }
    }
}

/// `n` random bytes from the OS source. When that is unreadable, a digest of the clock and a counter (weaker, but
/// never empty).
fn random_bytes(n: usize) -> Vec<u8> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut bytes = vec![0u8; n];
    if apz_os::random::fill(&mut bytes).is_err() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let seed = format!("{nanos}:{}", COUNTER.fetch_add(1, Ordering::Relaxed));
        let digest = Sha256::digest(seed.as_bytes());
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = digest[i % digest.len()];
        }
    }
    bytes
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn spec_key(id: &str, version: u32) -> String {
    format!("artifacts/{id}/v{version}.json")
}

fn blob_key(blob_id: &str) -> String {
    format!("artifacts/blobs/{blob_id}")
}

/// The key of script `path` of panel `panel`, compiled from version `version` of artifact `id`.
fn compiled_key(id: &str, version: u32, panel: &str, path: &str) -> String {
    format!("artifacts/{id}/v{version}/compiled/{panel}/{path}")
}

/// The extensions of the files the hub compiles.
const SCRIPT_EXTENSIONS: [&str; 5] = [".ts", ".tsx", ".jsx", ".js", ".mjs"];

/// The script files of some panels: each HTML body's files whose path ends in one of [`SCRIPT_EXTENSIONS`] and
/// that carry text, as `(panel id, path, text)`.
fn script_files(panels: &[Panel]) -> Vec<(&str, &str, &str)> {
    let mut out = vec![];
    for panel in panels {
        let Body::Html(h) = &panel.body else {
            continue;
        };
        for file in &h.files {
            let is_script = SCRIPT_EXTENSIONS.iter().any(|ext| file.path.ends_with(ext));
            if let (true, Some(text)) = (is_script, &file.text) {
                out.push((panel.id.as_str(), file.path.as_str(), text.as_str()));
            }
        }
    }
    out
}

/// Compiles every script of `spec` (see [`script_files`]) to `(panel, path, JavaScript)` triples. A script that does
/// not compile refuses the spec: each problem is `<panel>: <path>: <message>`.
fn compile_scripts(spec: &Spec) -> Result<Vec<(String, String, String)>, Vec<String>> {
    let mut out = vec![];
    let mut problems = vec![];
    for (panel, path, text) in script_files(&spec.panels) {
        match apz_artifact::compile(path, text) {
            Ok(js) => out.push((panel.to_string(), path.to_string(), js)),
            Err(messages) => problems.extend(
                messages
                    .into_iter()
                    .map(|m| format!("{panel}: {path}: {m}")),
            ),
        }
    }
    if problems.is_empty() {
        Ok(out)
    } else {
        Err(problems)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CSV: &str = "name,score\nada,9\n\"Grace, H\",12\nlin,10\n";

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("apid-artifacts-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    /// A store in `dir` over a booted vault and a local-disk deliverables store, as the hub has them at start.
    fn open(dir: &Path) -> (Store, Arc<deliverables::Store>) {
        let vault = crate::vault::Store::open(dir, &dir.join("vault.key"), None);
        vault.boot(apz_plaza::now_ms()).expect("boot");
        let objects = Arc::new(
            deliverables::Store::open(dir, &BTreeMap::new(), Arc::new(vault)).expect("objects"),
        );
        (Store::load(dir, objects.clone()), objects)
    }

    /// A one-panel dataset spec over blob `blob`.
    fn dataset_spec(title: &str, blob: &BlobRef) -> Spec {
        serde_json::from_value(json!({
            "title": title,
            "panels": [{
                "id": "data",
                "title": "Scores",
                "span": 12,
                "body": {
                    "type": "dataset",
                    "columns": [
                        {"key": "name", "label": "Name"},
                        {"key": "score", "label": "Score", "kind": "number"}
                    ],
                    "blob": {"id": blob.id, "content_type": blob.content_type, "bytes": blob.bytes}
                }
            }]
        }))
        .expect("spec")
    }

    #[test]
    fn create_update_and_versions() {
        let dir = scratch("versions");
        let (store, _) = open(&dir);
        let blob = store
            .put_blob(Some(1), "text/csv", CSV.as_bytes())
            .expect("blob");
        let first = store
            .create(
                "a1 · Ada",
                Some(1),
                Some("ops".into()),
                dataset_spec("Scores", &blob),
            )
            .expect("create");
        assert_eq!(
            (first.version, first.latest, first.kind.as_str()),
            (1, 1, "dataset")
        );
        assert!(first.bytes > blob.bytes);
        let id = first.id.clone();
        let second = store
            .update(&id, Some(1), dataset_spec("Scores v2", &blob))
            .expect("update");
        assert_eq!(second.version, 2);
        assert!(
            store
                .update(&id, Some(2), dataset_spec("Mine", &blob))
                .is_err()
        );
        assert_eq!(
            store.get(&id, None).map(|a| a.title),
            Some("Scores v2".to_string())
        );
        assert_eq!(
            store.get(&id, Some(1)).map(|a| a.title),
            Some("Scores".to_string())
        );
        assert_eq!(store.versions(&id).len(), 2);
        assert_eq!(store.author_cid(&id), Some(1));
        assert!(store.delete(&id));
        assert!(store.get(&id, None).is_none());
        assert!(!store.delete(&id));
    }

    #[test]
    fn refusals_name_the_problem() {
        let dir = scratch("refusals");
        let (store, _) = open(&dir);
        let blob = store
            .put_blob(Some(1), "text/csv", CSV.as_bytes())
            .expect("blob");
        let problems = store
            .create("a1", Some(1), None, dataset_spec("", &blob))
            .expect_err("an empty title is refused");
        assert!(
            problems.iter().any(|p| p.contains("title is empty")),
            "{problems:?}"
        );
        let missing = BlobRef {
            id: "b0000000000000000".into(),
            content_type: "text/csv".into(),
            bytes: 1,
        };
        let problems = store
            .create("a1", Some(1), None, dataset_spec("X", &missing))
            .expect_err("a missing blob is refused");
        assert!(
            problems.iter().any(|p| p.contains("does not exist")),
            "{problems:?}"
        );
        let theirs = store
            .put_blob(Some(2), "text/csv", CSV.as_bytes())
            .expect("blob");
        let problems = store
            .create("a1", Some(1), None, dataset_spec("X", &theirs))
            .expect_err("another agent's blob is refused");
        assert!(
            problems.iter().any(|p| p.contains("is not yours")),
            "{problems:?}"
        );
    }

    #[test]
    fn list_is_newest_first_and_filtered() {
        let dir = scratch("list");
        let (store, _) = open(&dir);
        let blob = store
            .put_blob(Some(1), "text/csv", CSV.as_bytes())
            .expect("blob");
        let first = store
            .create(
                "a1 · Ada",
                Some(1),
                Some("ops".into()),
                dataset_spec("Scores", &blob),
            )
            .expect("first");
        let second = store
            .create("a1 · Ada", Some(1), None, dataset_spec("Second", &blob))
            .expect("second");
        let ids: Vec<String> = store.list(None, None).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![second.id.clone(), first.id.clone()]);
        let in_ops: Vec<String> = store
            .list(Some("ops"), None)
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(in_ops, vec![first.id]);
        assert_eq!(store.list(None, Some("SECOND")).len(), 1);
    }

    #[test]
    fn rows_sort_filter_and_page() {
        let dir = scratch("rows");
        let (store, _) = open(&dir);
        let blob = store
            .put_blob(Some(1), "text/csv", CSV.as_bytes())
            .expect("blob");
        let made = store
            .create("a1", Some(1), None, dataset_spec("Rows", &blob))
            .expect("create");
        let desc = store
            .rows(&made.id, None, "data", 0, 10, Some(("score", true)), None)
            .expect("rows");
        assert_eq!(desc.total, 3);
        assert_eq!(desc.rows[0][0], json!("Grace, H"));
        assert_eq!(desc.rows[0][1], json!(12.0));
        let filtered = store
            .rows(&made.id, None, "data", 0, 10, None, Some("ADA"))
            .expect("filter");
        assert_eq!(filtered.total, 1);
        let paged = store
            .rows(&made.id, None, "data", 2, 1, Some(("name", false)), None)
            .expect("page");
        assert_eq!(paged.total, 3);
        assert_eq!(paged.rows, vec![vec![json!("lin"), json!(10.0)]]);
        assert!(
            store
                .rows(&made.id, None, "missing", 0, 10, None, None)
                .is_err()
        );
    }

    #[test]
    fn blobs_are_served_only_through_their_artifact() {
        let dir = scratch("blobs");
        let (store, _) = open(&dir);
        let blob = store
            .put_blob(Some(1), "text/csv", CSV.as_bytes())
            .expect("blob");
        let made = store
            .create("a1", Some(1), None, dataset_spec("Blobs", &blob))
            .expect("create");
        let (content_type, bytes) = store.blob(&blob.id).expect("blob");
        assert_eq!(content_type, "text/csv");
        assert_eq!(bytes, CSV.as_bytes());
        assert!(store.artifact_blob(&made.id, &blob.id).is_some());
        assert!(store.artifact_blob("a00000000000", &blob.id).is_none());
        let other = store
            .put_blob(Some(1), "text/csv", b"x\n1\n")
            .expect("blob");
        assert!(store.artifact_blob(&made.id, &other.id).is_none());
    }

    #[test]
    fn the_index_survives_a_reload_and_delete_drops_unused_blobs() {
        let dir = scratch("reload");
        let (store, objects) = open(&dir);
        let blob = store
            .put_blob(Some(1), "text/csv", CSV.as_bytes())
            .expect("blob");
        let kept = store
            .create("a1", Some(1), None, dataset_spec("Kept", &blob))
            .expect("kept");
        let reloaded = Store::load(&dir, objects.clone());
        assert_eq!(reloaded.list(None, None).len(), 1);
        assert_eq!(
            reloaded.get(&kept.id, None).map(|a| a.title),
            Some("Kept".to_string())
        );
        assert!(reloaded.delete(&kept.id));
        assert!(reloaded.blob(&blob.id).is_none());
        let (count, _) = stats_of(&lock(&reloaded.index), Some(1), 0);
        assert_eq!(count, 0);
    }

    #[test]
    fn a_spent_budget_refuses_the_write_under_the_lock() {
        let dir = scratch("budget");
        let (store, _) = open(&dir);
        let blob = store
            .put_blob(Some(1), "text/csv", CSV.as_bytes())
            .expect("blob");
        let spent = Budget {
            since_ms: 0,
            check: |_, _| Some("spent".to_string()),
        };
        assert!(matches!(
            store.create_within("ar1", "a1", Some(1), None, dataset_spec("X", &blob), &spent),
            Err(Refusal::Budget(_))
        ));
        assert!(store.get("ar1", None).is_none());
        assert!(matches!(
            store.put_blob_within(Some(1), "text/csv", b"x\n1\n", &spent),
            Err(Refusal::Budget(_))
        ));
    }

    #[test]
    fn an_unreadable_index_is_set_aside_not_overwritten() {
        let dir = scratch("corrupt");
        std::fs::write(dir.join(INDEX), b"{ not json").expect("write");
        let (store, _) = open(&dir);
        assert!(store.list(None, None).is_empty());
        assert!(!dir.join(INDEX).exists());
        let kept: Vec<String> = std::fs::read_dir(&dir)
            .expect("dir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("artifacts.json.corrupt-"))
            .collect();
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!(
            std::fs::read(dir.join(&kept[0])).expect("kept"),
            b"{ not json"
        );
    }

    #[test]
    fn a_content_type_with_a_line_break_falls_back() {
        assert_eq!(safe_content_type("text/csv"), "text/csv");
        assert_eq!(
            safe_content_type("text/plain; charset=utf-8"),
            "text/plain; charset=utf-8"
        );
        assert_eq!(safe_content_type("image/svg+xml"), "image/svg+xml");
        for bad in [
            "text/csv\r\nSet-Cookie: x=1",
            "text/csv\nX: y",
            "text/html; foo=bar",
            "text",
            "/csv",
            "a b/c",
            "",
        ] {
            assert_eq!(
                safe_content_type(bad),
                "application/octet-stream",
                "{bad:?}"
            );
        }
        assert_eq!(
            safe_content_type(&format!("a/{}", "b".repeat(120))),
            "application/octet-stream"
        );
        let dir = scratch("content-type");
        let (store, _) = open(&dir);
        let blob = store
            .put_blob(Some(1), "text/csv\r\nX: 1", b"x\n1\n")
            .expect("blob");
        assert_eq!(blob.content_type, "application/octet-stream");
    }

    /// A one-panel HTML spec whose entry is `App.tsx` with the given text.
    fn html_spec(text: &str) -> Spec {
        serde_json::from_value(json!({
            "title": "Page",
            "panels": [{
                "id": "app",
                "title": "App",
                "span": 12,
                "body": {
                    "type": "html",
                    "entry": "App.tsx",
                    "files": [{"path": "App.tsx", "text": text}]
                }
            }]
        }))
        .expect("spec")
    }

    #[test]
    fn a_tsx_syntax_error_refuses_the_spec_with_its_panel_and_file() {
        let dir = scratch("compile-refused");
        let (store, _) = open(&dir);
        let spec = html_spec("export default function App() {\n  return <div>;\n}\n");
        let problems = store
            .create("a1", Some(1), None, spec)
            .expect_err("a syntax error is refused");
        assert!(
            problems.iter().any(|p| p.starts_with("app: App.tsx: 3:")),
            "{problems:?}"
        );
        assert!(store.list(None, None).is_empty());
    }

    #[test]
    fn a_compiled_script_is_read_back_by_version_and_panel() {
        let dir = scratch("compile-ok");
        let (store, _) = open(&dir);
        let spec = html_spec("import React from \"react\";\nexport default () => <div>hi</div>;\n");
        let made = store.create("a1", Some(1), None, spec).expect("create");
        let js = store
            .compiled(&made.id, 1, "app", "App.tsx")
            .expect("compiled");
        assert!(js.contains("const React = window.React;"), "{js}");
        assert!(js.contains("React.createElement"), "{js}");
        assert!(store.compiled(&made.id, 2, "app", "App.tsx").is_none());
        assert!(store.compiled(&made.id, 1, "other", "App.tsx").is_none());
        assert!(store.compiled(&made.id, 1, "app", "missing.tsx").is_none());
        assert!(store.delete(&made.id));
        assert!(store.compiled(&made.id, 1, "app", "App.tsx").is_none());
    }
}
