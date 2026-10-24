//! The skill library (docs/design/skill-library.md): SKILL.md folders with their files, immutable versions checked by
//! `apz_skill`, bindings to roles or the whole plaza, a live version, sources (built here, uploaded, or from GitHub),
//! and an append-only history. `SkillStore` keeps the rows (`store_db`: typed tables in `plaza.db`); `Blobs` keeps the
//! file bytes by their BLAKE3 hash (`blobs`: the hub's object store, in `<state_dir>/blobs`). `Skills` is what the rest
//! of apid uses: the routes, the import and export paths, the agents' tools and the hub's MCP tools.

pub mod agent_tools;
pub mod blobs;
pub mod import;
pub mod routes;
pub mod store_db;

use std::sync::Arc;
use std::time::Duration;

use apz_cache::{Cache, Generations};
use apz_skill::{FileInfo, Finding, Report};
use serde::Serialize;
use serde_json::{Value, json};

pub const MAX_TITLE: usize = 120;
pub const MAX_NOTE: usize = 500;
pub const MAX_TAGS: usize = 16;
pub const MAX_TAG: usize = 32;
pub const MAX_SKILL_MD: usize = 200_000;
/// The skill-level limits the `size-limit` rule enforces (a version over them is refused, not just flagged).
pub const MAX_FILES: usize = 200;
pub const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024;

/// Where a skill came from (flattened into the skill's row: `source_kind` and the GitHub columns).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, apz_store::Table)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    Built,
    Upload,
    Github {
        repo: String,
        git_ref: Option<String>,
        path: String,
        commit: String,
    },
}

/// One skill (without its versions' contents), as its row of `skill`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, apz_store::Table)]
#[table(name = "skill")]
pub struct Skill {
    /// A slug: `^[a-z0-9][a-z0-9-]{0,62}$`.
    #[table(key)]
    pub id: String,
    /// The frontmatter name of the latest version (agents call the skill by it; unique among live skills).
    pub name: String,
    pub title: String,
    /// The frontmatter description of the latest version.
    pub description: String,
    #[table(list)]
    pub tags: Vec<String>,
    /// `role:<role>` and/or `plaza`, sorted.
    #[table(list)]
    pub bindings: Vec<String>,
    pub active_version: Option<u32>,
    pub latest_version: u32,
    #[table(flatten)]
    pub source: Source,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub archived_ms: Option<u64>,
}

/// One file of a version (its bytes are in `Blobs` under `hash`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillFile {
    pub path: String,
    /// BLAKE3, lower-case hex.
    pub hash: String,
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
    /// BLAKE3 of SKILL.md, lower-case hex.
    pub hash: String,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, apz_store::Column)]
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

/// One history row of `skill_event`: appended, never changed. `id` is the history's place (one stream for all skills).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, apz_store::Table)]
#[table(name = "skill_event")]
pub struct Event {
    #[table(key)]
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
    pub hash: String,
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
    fn create(
        &self,
        new: &NewSkill,
        first: &NewVersion,
        actor: &str,
        now_ms: u64,
    ) -> Result<Skill, StoreError>;
    /// Appends version `base + 1`; `Conflict` unless `base` is the latest. For a GitHub skill, `next.source_commit`
    /// also becomes the skill's `source.commit`.
    fn add_version(
        &self,
        id: &str,
        base: u32,
        next: &NewVersion,
        actor: &str,
        now_ms: u64,
    ) -> Result<VersionMeta, StoreError>;
    /// Title and tags (`Edit`).
    fn set_meta(
        &self,
        id: &str,
        title: &str,
        tags: &[String],
        actor: &str,
        now_ms: u64,
    ) -> Result<Skill, StoreError>;
    /// Replaces the bindings (`Bind`).
    fn set_bindings(
        &self,
        id: &str,
        bindings: &[String],
        actor: &str,
        now_ms: u64,
    ) -> Result<Skill, StoreError>;
    /// `Some(v)` publishes (the version must exist), `None` unpublishes.
    fn set_active(
        &self,
        id: &str,
        version: Option<u32>,
        actor: &str,
        now_ms: u64,
    ) -> Result<Skill, StoreError>;
    /// Restoring a skill whose name another live skill took meanwhile is a `Conflict`.
    fn set_archived(
        &self,
        id: &str,
        archived: bool,
        actor: &str,
        now_ms: u64,
    ) -> Result<Skill, StoreError>;
    /// Newest first, at most `limit`.
    fn events(&self, id: &str, limit: u32) -> Result<Vec<Event>, String>;
    /// The live, published skills holding `binding`, with their live version's meta, by name.
    fn bound(&self, binding: &str) -> Result<Vec<(Skill, VersionMeta)>, String>;
}

/// File bytes by their BLAKE3 hash (lower-case hex). `put` returns the hash and is idempotent; `get` refuses bytes that
/// do not hash to the one asked for.
pub trait Blobs: Send + Sync {
    fn put(&self, bytes: &[u8]) -> Result<String, String>;
    fn get(&self, hash: &str) -> Result<Vec<u8>, String>;
}

/// A file as a client or an import hands it in: its bytes, or (for an unchanged file in a new version) the hash of
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

/// The SKILL.md of the built-in show-your-work skill, which the hub ships with.
const BUILTIN_SHOW_YOUR_WORK: &str = include_str!("builtin/show-your-work/SKILL.md");

/// The built-in skills the hub seeds into the library, each by its slug and the SKILL.md it is made from.
const BUILTIN: &[(&str, &str)] = &[("show-your-work", BUILTIN_SHOW_YOUR_WORK)];

/// Imports each built-in skill the library does not hold yet: a built skill bound to the whole plaza, published at
/// once. A skill is seeded once. One the library holds, archived or not, is left as the owner left it, so a restart
/// never undoes an archive. Returns how many were imported.
pub fn seed_builtins(skills: &Skills, actor: &str, now_ms: u64) -> Result<usize, String> {
    let store = skills.store();
    let mut imported = 0;
    for &(id, skill_md) in BUILTIN {
        let held =
            store.get(id)?.is_some() || store.list(false)?.iter().any(|skill| skill.name == id);
        if held {
            continue;
        }
        let first = skills
            .prepare(
                skill_md.to_string(),
                Vec::new(),
                "built in with the hub".into(),
                None,
            )
            .map_err(|err| format!("{id}: {err:?}"))?;
        let new = NewSkill {
            id: id.into(),
            title: "Show your work".into(),
            tags: vec!["builtin".into()],
            bindings: vec!["plaza".into()],
            source: Source::Built,
        };
        let skill = store
            .create(&new, &first, actor, now_ms)
            .map_err(|err| format!("{id}: {err:?}"))?;
        skills
            .publish(id, skill.latest_version, actor, now_ms)
            .map_err(|err| format!("{id}: {err:?}"))?;
        imported += 1;
    }
    Ok(imported)
}

/// The generation of the library as a whole: bumped by every write of any skill (the sessions' skills sections read it).
pub const LIBRARY: &str = "skills";

/// The generation of one skill: bumped by every write of that skill (its summary is cached by it).
pub fn skill_key(id: &str) -> String {
    format!("skill:{id}")
}

/// A skill's summary is kept until its generation moves, or this long at most.
const SUMMARY_AGE: Duration = Duration::from_secs(24 * 3600);

/// The store with its writes counted in the generations: each write bumps the skill's generation and the library's, after
/// the write has committed (so a cache built before the write never stores over it, see `apz_cache`). Every write of the
/// library goes through this wrapper, so no write path can miss a bump.
struct Bumped {
    inner: Arc<dyn SkillStore>,
    gens: Arc<Generations<String>>,
}

impl Bumped {
    fn bump(&self, id: &str) {
        self.gens.bump(&skill_key(id));
        self.gens.bump(&LIBRARY.to_string());
    }
}

impl SkillStore for Bumped {
    fn list(&self, archived: bool) -> Result<Vec<Skill>, String> {
        self.inner.list(archived)
    }
    fn get(&self, id: &str) -> Result<Option<Skill>, String> {
        self.inner.get(id)
    }
    fn versions(&self, id: &str) -> Result<Vec<VersionMeta>, String> {
        self.inner.versions(id)
    }
    fn version(&self, id: &str, version: u32) -> Result<Option<Version>, String> {
        self.inner.version(id, version)
    }
    fn create(
        &self,
        new: &NewSkill,
        first: &NewVersion,
        actor: &str,
        now_ms: u64,
    ) -> Result<Skill, StoreError> {
        let skill = self.inner.create(new, first, actor, now_ms)?;
        self.bump(&skill.id);
        Ok(skill)
    }
    fn add_version(
        &self,
        id: &str,
        base: u32,
        next: &NewVersion,
        actor: &str,
        now_ms: u64,
    ) -> Result<VersionMeta, StoreError> {
        let meta = self.inner.add_version(id, base, next, actor, now_ms)?;
        self.bump(id);
        Ok(meta)
    }
    fn set_meta(
        &self,
        id: &str,
        title: &str,
        tags: &[String],
        actor: &str,
        now_ms: u64,
    ) -> Result<Skill, StoreError> {
        let skill = self.inner.set_meta(id, title, tags, actor, now_ms)?;
        self.bump(id);
        Ok(skill)
    }
    fn set_bindings(
        &self,
        id: &str,
        bindings: &[String],
        actor: &str,
        now_ms: u64,
    ) -> Result<Skill, StoreError> {
        let skill = self.inner.set_bindings(id, bindings, actor, now_ms)?;
        self.bump(id);
        Ok(skill)
    }
    fn set_active(
        &self,
        id: &str,
        version: Option<u32>,
        actor: &str,
        now_ms: u64,
    ) -> Result<Skill, StoreError> {
        let skill = self.inner.set_active(id, version, actor, now_ms)?;
        self.bump(id);
        Ok(skill)
    }
    fn set_archived(
        &self,
        id: &str,
        archived: bool,
        actor: &str,
        now_ms: u64,
    ) -> Result<Skill, StoreError> {
        let skill = self.inner.set_archived(id, archived, actor, now_ms)?;
        self.bump(id);
        Ok(skill)
    }
    fn events(&self, id: &str, limit: u32) -> Result<Vec<Event>, String> {
        self.inner.events(id, limit)
    }
    fn bound(&self, binding: &str) -> Result<Vec<(Skill, VersionMeta)>, String> {
        self.inner.bound(binding)
    }
}

/// The library as apid uses it.
pub struct Skills {
    store: Bumped,
    blobs: Arc<dyn Blobs>,
    gens: Arc<Generations<String>>,
    /// Each skill's summary, by skill id, built from its versions (see [`Skills::summary`]).
    summaries: Cache<String, Value>,
}

impl Skills {
    /// The library counting its writes in `gens`, the hub's generations.
    pub fn with_generations(
        store: Arc<dyn SkillStore>,
        blobs: Arc<dyn Blobs>,
        gens: Arc<Generations<String>>,
    ) -> Skills {
        Skills {
            store: Bumped {
                inner: store,
                gens: Arc::clone(&gens),
            },
            blobs,
            gens,
            summaries: Cache::new(SUMMARY_AGE),
        }
    }

    /// The summary of skill `id` (its fields, live version's score and best version), as the list and detail routes show
    /// it. Built from the store only when the skill changed since the last summary: a list of N skills reads the
    /// versions of each once, not on every list.
    pub fn summary(&self, id: &str) -> Result<Value, StoreError> {
        let value = self
            .summaries
            .get(
                &id.to_string(),
                || self.gens.get(&skill_key(id)),
                || self.build_summary(id),
            )
            .map_err(StoreError::Failed)?;
        Ok((*value).clone())
    }

    /// The summary read fresh from the store (after the generation was read, so a write that lands meanwhile is never
    /// missed: its bump moves the generation and this build is not stored).
    fn build_summary(&self, id: &str) -> Result<Value, String> {
        let skill = self
            .store
            .get(id)?
            .ok_or_else(|| format!("no skill {id}"))?;
        let versions = self.store.versions(id)?;
        let best = apz_prompt::best_version(versions.iter().map(|v| (v.version, v.score)));
        let score = skill
            .active_version
            .and_then(|live| versions.iter().find(|v| v.version == live))
            .map(|v| v.score);
        Ok(json!({
            "id": skill.id,
            "name": skill.name,
            "title": skill.title,
            "description": skill.description,
            "tags": skill.tags,
            "bindings": skill.bindings,
            "active_version": skill.active_version,
            "latest_version": skill.latest_version,
            "best_version": best,
            "score": score,
            "source": skill.source,
            "created_ms": skill.created_ms,
            "updated_ms": skill.updated_ms,
            "archived_ms": skill.archived_ms,
        }))
    }

    pub fn store(&self) -> &dyn SkillStore {
        &self.store
    }

    pub fn blobs(&self) -> &dyn Blobs {
        &*self.blobs
    }

    /// Checks SKILL.md and the files, stores the file bytes, and returns the version to write. Refuses (Invalid) a
    /// version over the size limits or with an unsafe path; findings of other rules are stored, not refused.
    pub fn prepare(
        &self,
        skill_md: String,
        files: Vec<FileIn>,
        note: String,
        source_commit: Option<String>,
    ) -> Result<NewVersion, StoreError> {
        if skill_md.chars().count() > MAX_SKILL_MD {
            return Err(StoreError::Invalid(format!(
                "SKILL.md is longer than {MAX_SKILL_MD} characters"
            )));
        }
        if note.chars().count() > MAX_NOTE {
            return Err(StoreError::Invalid(format!(
                "the note is longer than {MAX_NOTE} characters"
            )));
        }
        if files.len() > MAX_FILES {
            return Err(StoreError::Invalid(format!(
                "a skill holds at most {MAX_FILES} files"
            )));
        }
        let mut stored = Vec::with_capacity(files.len());
        let mut infos = Vec::with_capacity(files.len());
        let mut total = 0u64;
        for f in files {
            if !safe_path(&f.path) {
                return Err(StoreError::Invalid(format!(
                    "{}: not a relative path inside the skill",
                    f.path
                )));
            }
            let bytes = match f.body {
                FileBody::Bytes(b) => b,
                FileBody::Known(hash) => self
                    .blobs
                    .get(&hash)
                    .map_err(|_| StoreError::Invalid(format!("{}: unknown file {hash}", f.path)))?,
            };
            let size = bytes.len() as u64;
            if size > MAX_FILE_BYTES {
                return Err(StoreError::Invalid(format!(
                    "{} is larger than 5 MB",
                    f.path
                )));
            }
            total += size;
            if total > MAX_TOTAL_BYTES {
                return Err(StoreError::Invalid("the skill is larger than 20 MB".into()));
            }
            let text = (size <= 1024 * 1024)
                .then(|| String::from_utf8(bytes.clone()).ok())
                .flatten();
            let hash = self.blobs.put(&bytes)?;
            infos.push(FileInfo {
                path: f.path.clone(),
                size,
                executable: f.executable,
                text,
            });
            stored.push(SkillFile {
                path: f.path,
                hash,
                size,
                executable: f.executable,
            });
        }
        stored.sort_by(|a, b| a.path.cmp(&b.path));
        let report = apz_skill::check(&skill_md, &infos);
        let hash = apz_blobs::Hash::of(skill_md.as_bytes()).to_string();
        Ok(NewVersion {
            skill_md,
            hash,
            files: stored,
            note,
            report,
            source_commit,
        })
    }

    /// Publishes `version`; refused when it has error-level findings under the current rules.
    pub fn publish(
        &self,
        id: &str,
        version: u32,
        actor: &str,
        now_ms: u64,
    ) -> Result<Skill, StoreError> {
        let v = self
            .store
            .version(id, version)?
            .ok_or_else(|| StoreError::Invalid(format!("{id} has no version {version}")))?;
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
                let text = if f.size <= 1024 * 1024 {
                    self.blobs
                        .get(&f.hash)
                        .ok()
                        .and_then(|b| String::from_utf8(b).ok())
                } else {
                    None
                };
                Ok(FileInfo {
                    path: f.path.clone(),
                    size: f.size,
                    executable: f.executable,
                    text,
                })
            })
            .collect()
    }

    /// Every file hash of every version of every skill, archived or not, each once: the sweep's roots for skill files.
    /// A version that cannot be read is an error, so a sweep stops rather than free bytes a row still names.
    pub fn hashes(&self) -> Result<Vec<apz_blobs::Hash>, String> {
        let mut out = Vec::new();
        // `list(true)` includes the archived skills, so one listing covers them all.
        for skill in self.store().list(true)? {
            for meta in self.store().versions(&skill.id)? {
                let version = self
                    .store()
                    .version(&skill.id, meta.version)?
                    .ok_or_else(|| format!("{} has no version {}", skill.id, meta.version))?;
                for file in &version.files {
                    out.push(file.hash.parse::<apz_blobs::Hash>()?);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        Ok(out)
    }
}

/// A relative `/`-separated path inside a skill: no `..`, no leading `/`, no `\`, not empty, not SKILL.md.
pub fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 300
        && !path.starts_with('/')
        && !path.contains('\\')
        && path != "SKILL.md"
        && path
            .split('/')
            .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

pub fn valid_id(id: &str) -> bool {
    crate::prompts::valid_id(id)
}

/// `plaza`, `role:<role>` with a lower-case role id, or `project:<id>` (a project's skills).
pub fn valid_binding(binding: &str) -> bool {
    binding == "plaza"
        || crate::prompts::project_binding(binding)
        || binding.strip_prefix("role:").is_some_and(|r| {
            !r.is_empty()
                && r.len() <= 64
                && r.bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        })
}

#[cfg(test)]
mod tests;
