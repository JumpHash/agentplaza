//! The prompt library (docs/design/prompt-library.md): system prompts, AGENTS.md files and fragments, each with
//! immutable versions checked by `apz_prompt`, a live version, bindings to roles or the whole plaza, and an
//! append-only history. `PromptStore` is served by `plaza.db` (`store`, typed tables). `Prompts` is what the rest of
//! apid uses: the routes, the system-prompt builder (`bound_body`) and the built-in import (`seed`).

pub mod routes;
pub mod seed;
pub mod store;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use apz_prompt::{Finding, Kind, Report};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// The longest body a version may hold (characters), as migration 0006 checks.
pub const MAX_BODY: usize = 200_000;
pub const MAX_TITLE: usize = 120;
pub const MAX_DESCRIPTION: usize = 500;
pub const MAX_NOTE: usize = 500;
pub const MAX_TAGS: usize = 16;
pub const MAX_TAG: usize = 32;

/// One prompt (without its versions' bodies).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, apz_store::Table)]
#[table(name = "prompt")]
pub struct Prompt {
    /// A slug: `^[a-z0-9][a-z0-9-]{0,62}$`.
    #[table(key)]
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub description: String,
    #[table(list)]
    pub tags: Vec<String>,
    /// `role:<role>` (system only) or `plaza` (agents_md only).
    pub binding: Option<String>,
    /// The live version; `None` when unpublished.
    pub active_version: Option<u32>,
    /// The highest version number (every prompt has at least version 1).
    pub latest_version: u32,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub archived_ms: Option<u64>,
}

/// A version's record without its body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VersionMeta {
    pub version: u32,
    pub note: String,
    /// `owner`, `builtin`, or `agent:<cid>` (a proposal through the hub's MCP server).
    pub author: String,
    pub created_ms: u64,
    pub score: u8,
    pub errors: u32,
    pub warnings: u32,
    pub infos: u32,
    /// The rule set the score was computed with (`apz_prompt::RULES_VERSION` at the time).
    pub rules_version: u32,
    /// sha256 of the body, lower-case hex.
    pub sha256: String,
}

/// A whole version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Version {
    #[serde(flatten)]
    pub meta: VersionMeta,
    pub body: String,
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
    Archive,
    Restore,
}

/// One history row. Its id is its place in the history (the stream's key).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, apz_store::Table)]
#[table(name = "prompt_event")]
pub struct Event {
    #[table(key)]
    pub id: u64,
    #[table(index)]
    pub prompt_id: String,
    pub action: Action,
    /// The version created or published (none for edit, archive, restore, unpublish).
    pub version: Option<u32>,
    pub actor: String,
    pub at_ms: u64,
}

/// A new prompt's fields (validated by `validate_new` before a store sees it).
#[derive(Debug, Clone)]
pub struct NewPrompt {
    pub id: String,
    pub kind: Kind,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub binding: Option<String>,
}

/// The editable metadata.
#[derive(Debug, Clone)]
pub struct Meta {
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub binding: Option<String>,
}

/// A version to write: the body, its note, and its check report (computed by `Prompts`, never by a store).
#[derive(Debug, Clone)]
pub struct NewVersion {
    pub body: String,
    pub note: String,
    pub sha256: String,
    pub report: Report,
}

impl NewVersion {
    pub fn new(kind: Kind, body: String, note: String) -> NewVersion {
        let sha256 = sha256_hex(&body);
        let report = apz_prompt::check(kind, &body);
        NewVersion {
            body,
            note,
            sha256,
            report,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    NotFound,
    /// An id or a binding already taken, or a stale `base_version`; the message says which.
    Conflict(String),
    /// The request breaks a rule (a version with errors published, an unknown version).
    Invalid(String),
    /// The database or the file failed.
    Failed(String),
}

impl From<String> for StoreError {
    fn from(e: String) -> Self {
        StoreError::Failed(e)
    }
}

/// Where prompts are kept. Every write is atomic and writes its `Event` in the same transaction. The binding of a
/// live (not archived) prompt is unique: a write that would take a binding another live prompt holds is a
/// `Conflict`.
pub trait PromptStore: Send + Sync {
    /// Every prompt, by id; archived ones only when `archived` is true.
    fn list(&self, archived: bool) -> Result<Vec<Prompt>, String>;
    fn get(&self, id: &str) -> Result<Option<Prompt>, String>;
    /// The versions of `id`, oldest first. Empty for an unknown id.
    fn versions(&self, id: &str) -> Result<Vec<VersionMeta>, String>;
    fn version(&self, id: &str, version: u32) -> Result<Option<Version>, String>;
    /// Creates the prompt and its version 1 (`Create` event, then `Version` event). Unpublished.
    fn create(
        &self,
        new: &NewPrompt,
        first: &NewVersion,
        actor: &str,
        now_ms: u64,
    ) -> Result<Prompt, StoreError>;
    /// Appends version `base + 1`. `Conflict` unless `base` is the latest version.
    fn add_version(
        &self,
        id: &str,
        base: u32,
        next: &NewVersion,
        actor: &str,
        now_ms: u64,
    ) -> Result<VersionMeta, StoreError>;
    /// `Edit` event.
    fn set_meta(
        &self,
        id: &str,
        meta: &Meta,
        actor: &str,
        now_ms: u64,
    ) -> Result<Prompt, StoreError>;
    /// `Some(v)` publishes version v (`Publish`), `None` unpublishes (`Unpublish`). The store checks that the version
    /// exists; `Prompts` checks it has no errors.
    fn set_active(
        &self,
        id: &str,
        version: Option<u32>,
        actor: &str,
        now_ms: u64,
    ) -> Result<Prompt, StoreError>;
    /// `Archive` or `Restore`. Restoring a prompt whose binding another live prompt took meanwhile is a `Conflict`.
    fn set_archived(
        &self,
        id: &str,
        archived: bool,
        actor: &str,
        now_ms: u64,
    ) -> Result<Prompt, StoreError>;
    /// The history of `id`, newest first, at most `limit` rows.
    fn events(&self, id: &str, limit: u32) -> Result<Vec<Event>, String>;
    /// The live prompt holding `binding` that has a published version, with that version's body.
    fn bound(&self, binding: &str) -> Result<Option<(Prompt, String)>, String>;
}

/// The library as apid uses it: a store plus a cache of bound bodies for the system-prompt builder, which runs on
/// every agent turn. Every write through `Prompts` clears the cache.
pub struct Prompts {
    store: Arc<dyn PromptStore>,
    bound_cache: Mutex<HashMap<String, Option<String>>>,
}

impl Prompts {
    pub fn new(store: Arc<dyn PromptStore>) -> Prompts {
        Prompts {
            store,
            bound_cache: Mutex::new(HashMap::new()),
        }
    }

    pub fn store(&self) -> &dyn PromptStore {
        &*self.store
    }

    /// Clears the bound-body cache; every write calls it.
    pub fn invalidate(&self) {
        self.bound_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    /// The published body bound to `binding` (`role:<role>` or `plaza`), cached. A store error is logged and reads
    /// as none, so an agent falls back to the built-in prompt rather than failing its turn.
    pub fn bound_body(&self, binding: &str) -> Option<String> {
        if let Some(hit) = self
            .bound_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(binding)
        {
            return hit.clone();
        }
        let body = match self.store.bound(binding) {
            Ok(found) => found.map(|(_, body)| body),
            Err(err) => {
                eprintln!(
                    "apid: prompt library: reading the prompt bound to {binding} failed: {err}"
                );
                return None;
            }
        };
        self.bound_cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(binding.to_string(), body.clone());
        body
    }

    pub fn create(
        &self,
        new: &NewPrompt,
        body: String,
        note: String,
        actor: &str,
        now_ms: u64,
    ) -> Result<Prompt, StoreError> {
        validate_new(new)?;
        validate_body(&body, &note)?;
        let first = NewVersion::new(new.kind, body, note);
        let out = self.store.create(new, &first, actor, now_ms);
        self.invalidate();
        out
    }

    /// Adds a version, or returns the latest one unchanged (`false`) when the body equals it.
    pub fn add_version(
        &self,
        id: &str,
        base: u32,
        body: String,
        note: String,
        actor: &str,
        now_ms: u64,
    ) -> Result<(VersionMeta, bool), StoreError> {
        validate_body(&body, &note)?;
        let prompt = self.store.get(id)?.ok_or(StoreError::NotFound)?;
        let latest = self.store.versions(id)?.pop().ok_or(StoreError::NotFound)?;
        let next = NewVersion::new(prompt.kind, body, note);
        if next.sha256 == latest.sha256 {
            return Ok((latest, false));
        }
        let out = self
            .store
            .add_version(id, base, &next, actor, now_ms)
            .map(|v| (v, true));
        self.invalidate();
        out
    }

    pub fn set_meta(
        &self,
        id: &str,
        meta: &Meta,
        actor: &str,
        now_ms: u64,
    ) -> Result<Prompt, StoreError> {
        let prompt = self.store.get(id)?.ok_or(StoreError::NotFound)?;
        validate_meta(
            prompt.kind,
            &meta.title,
            &meta.description,
            &meta.tags,
            meta.binding.as_deref(),
        )?;
        let out = self.store.set_meta(id, meta, actor, now_ms);
        self.invalidate();
        out
    }

    /// Publishes `version`; refused when it has error-level findings under the current rules.
    pub fn publish(
        &self,
        id: &str,
        version: u32,
        actor: &str,
        now_ms: u64,
    ) -> Result<Prompt, StoreError> {
        let prompt = self.store.get(id)?.ok_or(StoreError::NotFound)?;
        let v = self
            .store
            .version(id, version)?
            .ok_or_else(|| StoreError::Invalid(format!("{id} has no version {version}")))?;
        let report = apz_prompt::check(prompt.kind, &v.body);
        if !report.publishable() {
            return Err(StoreError::Invalid(format!(
                "version {version} has {} error(s); fix them in a new version before publishing",
                report.errors
            )));
        }
        let out = self.store.set_active(id, Some(version), actor, now_ms);
        self.invalidate();
        out
    }

    pub fn unpublish(&self, id: &str, actor: &str, now_ms: u64) -> Result<Prompt, StoreError> {
        let out = self.store.set_active(id, None, actor, now_ms);
        self.invalidate();
        out
    }

    pub fn set_archived(
        &self,
        id: &str,
        archived: bool,
        actor: &str,
        now_ms: u64,
    ) -> Result<Prompt, StoreError> {
        let out = self.store.set_archived(id, archived, actor, now_ms);
        self.invalidate();
        out
    }
}

pub fn sha256_hex(body: &str) -> String {
    Sha256::digest(body.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn valid_id(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// A binding allowed for `kind`: `role:<role>` for system prompts, `plaza` or `project:<id>` (a project's rules)
/// for AGENTS.md, none for fragments.
pub fn valid_binding(kind: Kind, binding: &str) -> bool {
    match kind {
        Kind::System => binding.strip_prefix("role:").is_some_and(|r| {
            !r.is_empty()
                && r.len() <= 64
                && r.bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        }),
        Kind::AgentsMd => binding == "plaza" || project_binding(binding),
        Kind::Fragment => false,
    }
}

/// Whether `binding` is a project's: `project:` and a project id (`pr<n>`, lower-case letters, digits and dashes).
pub fn project_binding(binding: &str) -> bool {
    binding.strip_prefix("project:").is_some_and(|id| {
        !id.is_empty()
            && id.len() <= 64
            && id
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    })
}

fn invalid(msg: impl Into<String>) -> StoreError {
    StoreError::Invalid(msg.into())
}

pub fn validate_new(new: &NewPrompt) -> Result<(), StoreError> {
    if !valid_id(&new.id) {
        return Err(invalid(
            "id must be 1-63 lower-case letters, digits and dashes, starting with a letter or digit",
        ));
    }
    validate_meta(
        new.kind,
        &new.title,
        &new.description,
        &new.tags,
        new.binding.as_deref(),
    )
}

pub fn validate_meta(
    kind: Kind,
    title: &str,
    description: &str,
    tags: &[String],
    binding: Option<&str>,
) -> Result<(), StoreError> {
    let title_len = title.trim().chars().count();
    if title_len == 0 || title_len > MAX_TITLE {
        return Err(invalid(format!("title must be 1-{MAX_TITLE} characters")));
    }
    if description.chars().count() > MAX_DESCRIPTION {
        return Err(invalid(format!(
            "description is longer than {MAX_DESCRIPTION} characters"
        )));
    }
    if tags.len() > MAX_TAGS
        || tags
            .iter()
            .any(|t| t.trim().is_empty() || t.chars().count() > MAX_TAG)
    {
        return Err(invalid(format!(
            "at most {MAX_TAGS} tags of 1-{MAX_TAG} characters"
        )));
    }
    if let Some(b) = binding
        && !valid_binding(kind, b)
    {
        return Err(invalid(format!(
            "{} prompts cannot bind to {b}",
            kind.as_str()
        )));
    }
    Ok(())
}

pub fn validate_body(body: &str, note: &str) -> Result<(), StoreError> {
    if body.chars().count() > MAX_BODY {
        return Err(invalid(format!(
            "the body is longer than {MAX_BODY} characters"
        )));
    }
    if note.chars().count() > MAX_NOTE {
        return Err(invalid(format!(
            "the note is longer than {MAX_NOTE} characters"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
