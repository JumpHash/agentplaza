//! The skill library (docs/design/skill-library.md): SKILL.md folders with their files, immutable versions checked by
//! `apz_skill`, bindings to roles or the whole plaza, a live version, sources (built here, uploaded, or from GitHub),
//! and an append-only history. `SkillStore` keeps the rows (Postgres: `store_db`, migration 0007; or `store_mem`,
//! `<state_dir>/skills.json`); `Blobs` keeps the file bytes by sha256 (Garage through the Library's store, or
//! `<state_dir>/skills/blobs`). `Skills` is what the rest of apid uses: the routes, the import and export paths, the
//! agents' tools and the hub's MCP tools.

pub mod agent_tools;
pub mod blobs;
pub mod import;
pub mod routes;
pub mod store_db;
pub mod store_mem;

use std::sync::Arc;

use apz_skill::{FileInfo, Finding, Report};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub const MAX_TITLE: usize = 120;
pub const MAX_NOTE: usize = 500;
pub const MAX_TAGS: usize = 16;
pub const MAX_TAG: usize = 32;
pub const MAX_SKILL_MD: usize = 200_000;
/// The skill-level limits the `size-limit` rule enforces (a version over them is refused, not just flagged).
pub const MAX_FILES: usize = 200;
pub const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024;

/// Where a skill came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    Built,
    Upload,
    Github { repo: String, git_ref: Option<String>, path: String, commit: String },
}

impl Source {
    pub fn kind(&self) -> &'static str {
        match self {
            Source::Built => "built",
            Source::Upload => "upload",
            Source::Github { .. } => "github",
        }
    }
}

/// One skill (without its versions' contents).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Skill {
    /// A slug: `^[a-z0-9][a-z0-9-]{0,62}$`.
    pub id: String,
    /// The frontmatter name of the latest version (agents call the skill by it; unique among live skills).
    pub name: String,
    pub title: String,
    /// The frontmatter description of the latest version.
    pub description: String,
    pub tags: Vec<String>,
    /// `role:<role>` and/or `plaza`, sorted.
    pub bindings: Vec<String>,
    pub active_version: Option<u32>,
    pub latest_version: u32,
    pub source: Source,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub archived_ms: Option<u64>,
}

/// One file of a version (its bytes are in `Blobs` under `sha256`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillFile {
    pub path: String,
    /// Lower-case hex.
    pub sha256: String,
    pub size: u64,
    pub executable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VersionMeta {
    pub version: u32,
    pub note: String,
    /// `owner`, `github`, `mcp`, or `agent:<cid>`.
    pub author: String,
    pub created_ms: u64,
    pub score: u8,
    pub errors: u32,
    pub warnings: u32,
    pub infos: u32,
    pub rules_version: u32,
    /// sha256 of SKILL.md, lower-case hex.
    pub sha256: String,
    pub file_count: u32,
    pub total_bytes: u64,
    /// For a GitHub skill, the commit this version was taken from.
    pub source_commit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Version {
    #[serde(flatten)]
    pub meta: VersionMeta,
    pub skill_md: String,
    pub files: Vec<SkillFile>,
    /// As stored when the version was written (under `meta.rules_version`).
    pub findings: Vec<Finding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Create,
    Version,
    Publish,
    Unpublish,
    Edit,
    Bind,
    Archive,
    Restore,
}

impl Action {
    pub const ALL: [Action; 8] = [
        Action::Create,
        Action::Version,
        Action::Publish,
        Action::Unpublish,
        Action::Edit,
        Action::Bind,
        Action::Archive,
        Action::Restore,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Action::Create => "create",
            Action::Version => "version",
            Action::Publish => "publish",
            Action::Unpublish => "unpublish",
            Action::Edit => "edit",
            Action::Bind => "bind",
            Action::Archive => "archive",
            Action::Restore => "restore",
        }
    }

    pub fn parse(s: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.as_str() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Event {
    pub id: u64,
    pub skill_id: String,
    pub action: Action,
    pub version: Option<u32>,
    pub actor: String,
    pub at_ms: u64,
}

/// A new skill's fields.
#[derive(Debug, Clone)]
pub struct NewSkill {
    pub id: String,
    pub title: String,
    pub tags: Vec<String>,
    pub bindings: Vec<String>,
    pub source: Source,
}

/// A version to write, already checked and with its files already in `Blobs` (`Skills` builds it, never a store).
#[derive(Debug, Clone)]
pub struct NewVersion {
    pub skill_md: String,
    pub sha256: String,
    pub files: Vec<SkillFile>,
    pub note: String,
    pub report: Report,
    pub source_commit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    NotFound,
    /// An id or a live name already taken, or a stale `base_version`.
    Conflict(String),
    Invalid(String),
    Failed(String),
}

impl From<String> for StoreError {
    fn from(e: String) -> Self {
        StoreError::Failed(e)
    }
}

/// Where skill rows are kept. Every write is atomic with its event. A live (not archived) skill's `name` is unique: a
/// write that would give a live skill a name another live skill holds is a `Conflict`. `name` and `description` on
/// the skill follow the latest version's frontmatter.
pub trait SkillStore: Send + Sync {
    fn list(&self, archived: bool) -> Result<Vec<Skill>, String>;
    fn get(&self, id: &str) -> Result<Option<Skill>, String>;
    /// Oldest first; empty for an unknown id.
    fn versions(&self, id: &str) -> Result<Vec<VersionMeta>, String>;
    fn version(&self, id: &str, version: u32) -> Result<Option<Version>, String>;
    /// Creates the skill with version 1 (`Create` then `Version` events). Unpublished.
    fn create(&self, new: &NewSkill, first: &NewVersion, actor: &str, now_ms: u64) -> Result<Skill, StoreError>;
    /// Appends version `base + 1`; `Conflict` unless `base` is the latest. For a GitHub skill, `next.source_commit`
    /// also becomes the skill's `source.commit`.
    fn add_version(&self, id: &str, base: u32, next: &NewVersion, actor: &str, now_ms: u64)
        -> Result<VersionMeta, StoreError>;
    /// Title and tags (`Edit`).
    fn set_meta(&self, id: &str, title: &str, tags: &[String], actor: &str, now_ms: u64) -> Result<Skill, StoreError>;
    /// Replaces the bindings (`Bind`).
    fn set_bindings(&self, id: &str, bindings: &[String], actor: &str, now_ms: u64) -> Result<Skill, StoreError>;
    /// `Some(v)` publishes (the version must exist), `None` unpublishes.
    fn set_active(&self, id: &str, version: Option<u32>, actor: &str, now_ms: u64) -> Result<Skill, StoreError>;
    /// Restoring a skill whose name another live skill took meanwhile is a `Conflict`.
    fn set_archived(&self, id: &str, archived: bool, actor: &str, now_ms: u64) -> Result<Skill, StoreError>;
    /// Newest first, at most `limit`.
    fn events(&self, id: &str, limit: u32) -> Result<Vec<Event>, String>;
    /// The live, published skills holding `binding`, with their live version's meta, by name.
    fn bound(&self, binding: &str) -> Result<Vec<(Skill, VersionMeta)>, String>;
}

/// File bytes by sha256 (lower-case hex). `put` returns the sha256 and is idempotent.
pub trait Blobs: Send + Sync {
    fn put(&self, bytes: &[u8]) -> Result<String, String>;
    fn get(&self, sha256: &str) -> Result<Vec<u8>, String>;
}

/// A file as a client or an import hands it in: its bytes, or (for an unchanged file in a new version) the sha256 of
/// bytes already in `Blobs`.
#[derive(Debug, Clone)]
pub enum FileBody {
    Bytes(Vec<u8>),
    Known(String),
}

#[derive(Debug, Clone)]
pub struct FileIn {
    pub path: String,
    pub body: FileBody,
    pub executable: bool,
}

/// The library as apid uses it.
pub struct Skills {
    store: Arc<dyn SkillStore>,
    blobs: Arc<dyn Blobs>,
}

impl Skills {
    pub fn new(store: Arc<dyn SkillStore>, blobs: Arc<dyn Blobs>) -> Skills {
        Skills { store, blobs }
    }

    pub fn store(&self) -> &dyn SkillStore {
        &*self.store
    }

    pub fn blobs(&self) -> &dyn Blobs {
        &*self.blobs
    }

    /// Checks SKILL.md and the files, stores the file bytes, and returns the version to write. Refuses (Invalid) a
    /// version over the size limits or with an unsafe path; findings of other rules are stored, not refused.
    pub fn prepare(&self, skill_md: String, files: Vec<FileIn>, note: String, source_commit: Option<String>)
        -> Result<NewVersion, StoreError> {
        if skill_md.chars().count() > MAX_SKILL_MD {
            return Err(StoreError::Invalid(format!("SKILL.md is longer than {MAX_SKILL_MD} characters")));
        }
        if note.chars().count() > MAX_NOTE {
            return Err(StoreError::Invalid(format!("the note is longer than {MAX_NOTE} characters")));
        }
        if files.len() > MAX_FILES {
            return Err(StoreError::Invalid(format!("a skill holds at most {MAX_FILES} files")));
        }
        let mut stored = Vec::with_capacity(files.len());
        let mut infos = Vec::with_capacity(files.len());
        let mut total = 0u64;
        for f in files {
            if !safe_path(&f.path) {
                return Err(StoreError::Invalid(format!("{}: not a relative path inside the skill", f.path)));
            }
            let bytes = match f.body {
                FileBody::Bytes(b) => b,
                FileBody::Known(sha) => self.blobs.get(&sha).map_err(|_| StoreError::Invalid(format!("{}: unknown file {sha}", f.path)))?,
            };
            let size = bytes.len() as u64;
            if size > MAX_FILE_BYTES {
                return Err(StoreError::Invalid(format!("{} is larger than 5 MB", f.path)));
            }
            total += size;
            if total > MAX_TOTAL_BYTES {
                return Err(StoreError::Invalid("the skill is larger than 20 MB".into()));
            }
            let text = (size <= 1024 * 1024).then(|| String::from_utf8(bytes.clone()).ok()).flatten();
            let sha256 = self.blobs.put(&bytes)?;
            infos.push(FileInfo { path: f.path.clone(), size, executable: f.executable, text });
            stored.push(SkillFile { path: f.path, sha256, size, executable: f.executable });
        }
        stored.sort_by(|a, b| a.path.cmp(&b.path));
        let report = apz_skill::check(&skill_md, &infos);
        let sha256 = sha256_hex(skill_md.as_bytes());
        Ok(NewVersion { skill_md, sha256, files: stored, note, report, source_commit })
    }

    /// Publishes `version`; refused when it has error-level findings under the current rules.
    pub fn publish(&self, id: &str, version: u32, actor: &str, now_ms: u64) -> Result<Skill, StoreError> {
        let v = self.store.version(id, version)?.ok_or_else(|| StoreError::Invalid(format!("{id} has no version {version}")))?;
        let infos = self.file_infos(&v.files)?;
        let report = apz_skill::check(&v.skill_md, &infos);
        if !report.publishable() {
            return Err(StoreError::Invalid(format!(
                "version {version} has {} error(s); fix them in a new version before publishing",
                report.errors
            )));
        }
        self.store.set_active(id, Some(version), actor, now_ms)
    }

    /// The `FileInfo`s of stored files (reading text files back for the checks).
    pub fn file_infos(&self, files: &[SkillFile]) -> Result<Vec<FileInfo>, StoreError> {
        files
            .iter()
            .map(|f| {
                let text = if f.size <= 1024 * 1024 { self.blobs.get(&f.sha256).ok().and_then(|b| String::from_utf8(b).ok()) } else { None };
                Ok(FileInfo { path: f.path.clone(), size: f.size, executable: f.executable, text })
            })
            .collect()
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// A relative `/`-separated path inside a skill: no `..`, no leading `/`, no `\`, not empty, not SKILL.md.
pub fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 300
        && !path.starts_with('/')
        && !path.contains('\\')
        && path != "SKILL.md"
        && path.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

pub fn valid_id(id: &str) -> bool {
    crate::prompts::valid_id(id)
}

/// `plaza`, or `role:<role>` with a lower-case role id.
pub fn valid_binding(binding: &str) -> bool {
    binding == "plaza"
        || binding.strip_prefix("role:").is_some_and(|r| {
            !r.is_empty() && r.len() <= 64 && r.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        })
}

#[cfg(test)]
mod tests;
