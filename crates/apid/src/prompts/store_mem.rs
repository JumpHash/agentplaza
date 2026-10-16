//! `PromptStore` without Postgres: everything in memory, persisted to `<state_dir>/prompts.json` (written to a
//! temporary file and renamed over the old one after every write). The same rules as `store_db`.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use apz_prompt::{Finding, Kind};
use serde::{Deserialize, Serialize};

use super::{
    Action, Event, Meta, NewPrompt, NewVersion, Prompt, PromptStore, StoreError, Version,
    VersionMeta,
};

pub struct MemoryPrompts {
    /// `None` when nothing is persisted.
    path: Option<PathBuf>,
    inner: Mutex<Data>,
}

/// What the file holds.
#[derive(Clone, Default, Serialize, Deserialize)]
struct Data {
    /// By id.
    prompts: BTreeMap<String, Stored>,
    /// Every event of every prompt, oldest first.
    events: Vec<StoredEvent>,
    /// The id of the last event written; the next one is this plus one.
    last_event: u64,
}

#[derive(Clone, Serialize, Deserialize)]
struct Stored {
    id: String,
    kind: Kind,
    title: String,
    description: String,
    tags: Vec<String>,
    binding: Option<String>,
    active_version: Option<u32>,
    created_ms: u64,
    updated_ms: u64,
    archived_ms: Option<u64>,
    /// Oldest first: version `n` is at index `n - 1`.
    versions: Vec<StoredVersion>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredVersion {
    version: u32,
    note: String,
    author: String,
    created_ms: u64,
    score: u8,
    errors: u32,
    warnings: u32,
    infos: u32,
    rules_version: u32,
    sha256: String,
    body: String,
    findings: Vec<Finding>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredEvent {
    id: u64,
    prompt_id: String,
    /// `Action::as_str`.
    action: String,
    version: Option<u32>,
    actor: String,
    at_ms: u64,
}

impl Data {
    /// The live prompt, other than `except`, that holds `binding`.
    fn holder(&self, binding: &str, except: &str) -> Option<&str> {
        self.prompts
            .values()
            .find(|p| p.live() && p.id != except && p.binding.as_deref() == Some(binding))
            .map(|p| p.id.as_str())
    }

    fn prompt_mut(&mut self, id: &str) -> Result<&mut Stored, StoreError> {
        self.prompts.get_mut(id).ok_or(StoreError::NotFound)
    }

    fn log(
        &mut self,
        prompt_id: &str,
        action: Action,
        version: Option<u32>,
        actor: &str,
        at_ms: u64,
    ) {
        self.last_event += 1;
        self.events.push(StoredEvent {
            id: self.last_event,
            prompt_id: prompt_id.to_string(),
            action: action.as_str().to_string(),
            version,
            actor: actor.to_string(),
            at_ms,
        });
    }
}

impl Stored {
    fn live(&self) -> bool {
        self.archived_ms.is_none()
    }

    fn prompt(&self) -> Prompt {
        Prompt {
            id: self.id.clone(),
            kind: self.kind,
            title: self.title.clone(),
            description: self.description.clone(),
            tags: self.tags.clone(),
            binding: self.binding.clone(),
            active_version: self.active_version,
            latest_version: self.versions.len() as u32,
            created_ms: self.created_ms,
            updated_ms: self.updated_ms,
            archived_ms: self.archived_ms,
        }
    }
}

impl StoredVersion {
    fn new(version: u32, next: &NewVersion, author: &str, now_ms: u64) -> StoredVersion {
        StoredVersion {
            version,
            note: next.note.clone(),
            author: author.to_string(),
            created_ms: now_ms,
            score: next.report.score,
            errors: next.report.errors,
            warnings: next.report.warnings,
            infos: next.report.infos,
            rules_version: next.report.rules_version,
            sha256: next.sha256.clone(),
            body: next.body.clone(),
            findings: next.report.findings.clone(),
        }
    }

    fn meta(&self) -> VersionMeta {
        VersionMeta {
            version: self.version,
            note: self.note.clone(),
            author: self.author.clone(),
            created_ms: self.created_ms,
            score: self.score,
            errors: self.errors,
            warnings: self.warnings,
            infos: self.infos,
            rules_version: self.rules_version,
            sha256: self.sha256.clone(),
        }
    }

    fn full(&self) -> Version {
        Version {
            meta: self.meta(),
            body: self.body.clone(),
            findings: self.findings.clone(),
        }
    }
}

impl MemoryPrompts {
    /// Not persisted (tests).
    #[cfg(test)]
    pub fn in_memory() -> MemoryPrompts {
        MemoryPrompts {
            path: None,
            inner: Mutex::new(Data::default()),
        }
    }

    /// Loads `<state_dir>/prompts.json` if it exists, and writes it after every change.
    pub fn open(state_dir: &Path) -> MemoryPrompts {
        let path = state_dir.join("prompts.json");
        let data = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Data>(&bytes).unwrap_or_else(|err| {
                eprintln!(
                    "apid: prompt library: {} does not parse ({err}); starting empty, the file is kept as prompts.json.bad",
                    path.display()
                );
                if let Err(e) = std::fs::rename(&path, state_dir.join("prompts.json.bad")) {
                    eprintln!("apid: prompt library: keeping the unreadable file failed: {e}");
                }
                Data::default()
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Data::default(),
            Err(err) => {
                // The file is there but unreadable: never overwrite it. This run keeps the library in memory only.
                eprintln!(
                    "apid: prompt library: reading {} failed ({err}); the library stays in memory this run and the file is left as it is",
                    path.display()
                );
                return MemoryPrompts { path: None, inner: Mutex::new(Data::default()) };
            }
        };
        MemoryPrompts {
            path: Some(path),
            inner: Mutex::new(data),
        }
    }

    /// Applies `change` to a copy of the data, persists the copy, and only then makes it the live data: a failed
    /// check or a failed write leaves everything as it was.
    fn write<T>(
        &self,
        change: impl FnOnce(&mut Data) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let mut guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let mut next = guard.clone();
        let out = change(&mut next)?;
        self.persist(&next)?;
        *guard = next;
        Ok(out)
    }

    fn read<T>(&self, look: impl FnOnce(&Data) -> T) -> T {
        let guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        look(&guard)
    }

    fn persist(&self, data: &Data) -> Result<(), StoreError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let bytes = serde_json::to_vec_pretty(data)
            .map_err(|e| StoreError::Failed(format!("encoding the prompt library: {e}")))?;
        let tmp = path.with_extension("json.tmp");
        let written = std::fs::File::create(&tmp)
            .and_then(|mut f| {
                f.write_all(&bytes)?;
                f.sync_all()
            })
            .and_then(|()| std::fs::rename(&tmp, path));
        written.map_err(|e| StoreError::Failed(format!("writing {}: {e}", path.display())))
    }
}

impl PromptStore for MemoryPrompts {
    fn list(&self, archived: bool) -> Result<Vec<Prompt>, String> {
        Ok(self.read(|d| {
            d.prompts
                .values()
                .filter(|p| archived || p.live())
                .map(Stored::prompt)
                .collect()
        }))
    }

    fn get(&self, id: &str) -> Result<Option<Prompt>, String> {
        Ok(self.read(|d| d.prompts.get(id).map(Stored::prompt)))
    }

    fn versions(&self, id: &str) -> Result<Vec<VersionMeta>, String> {
        Ok(self.read(|d| {
            d.prompts
                .get(id)
                .map(|p| p.versions.iter().map(StoredVersion::meta).collect())
                .unwrap_or_default()
        }))
    }

    fn version(&self, id: &str, version: u32) -> Result<Option<Version>, String> {
        Ok(self.read(|d| {
            let v = d
                .prompts
                .get(id)?
                .versions
                .iter()
                .find(|v| v.version == version)?;
            Some(v.full())
        }))
    }

    fn create(
        &self,
        new: &NewPrompt,
        first: &NewVersion,
        actor: &str,
        now_ms: u64,
    ) -> Result<Prompt, StoreError> {
        self.write(|d| {
            if d.prompts.contains_key(&new.id) {
                return Err(StoreError::Conflict(format!(
                    "a prompt named {} exists",
                    new.id
                )));
            }
            if let Some(b) = new.binding.as_deref()
                && let Some(other) = d.holder(b, &new.id)
            {
                return Err(StoreError::Conflict(format!("{b} is bound to {other}")));
            }
            let stored = Stored {
                id: new.id.clone(),
                kind: new.kind,
                title: new.title.clone(),
                description: new.description.clone(),
                tags: new.tags.clone(),
                binding: new.binding.clone(),
                active_version: None,
                created_ms: now_ms,
                updated_ms: now_ms,
                archived_ms: None,
                versions: vec![StoredVersion::new(1, first, actor, now_ms)],
            };
            let prompt = stored.prompt();
            d.prompts.insert(new.id.clone(), stored);
            d.log(&new.id, Action::Create, None, actor, now_ms);
            d.log(&new.id, Action::Version, Some(1), actor, now_ms);
            Ok(prompt)
        })
    }

    fn add_version(
        &self,
        id: &str,
        base: u32,
        next: &NewVersion,
        actor: &str,
        now_ms: u64,
    ) -> Result<VersionMeta, StoreError> {
        self.write(|d| {
            let p = d.prompt_mut(id)?;
            let latest = p.versions.len() as u32;
            if base != latest {
                return Err(StoreError::Conflict(format!(
                    "{id} is at version {latest}, not {base}"
                )));
            }
            let v = StoredVersion::new(latest + 1, next, actor, now_ms);
            let meta = v.meta();
            p.versions.push(v);
            p.updated_ms = now_ms;
            d.log(id, Action::Version, Some(latest + 1), actor, now_ms);
            Ok(meta)
        })
    }

    fn set_meta(
        &self,
        id: &str,
        meta: &Meta,
        actor: &str,
        now_ms: u64,
    ) -> Result<Prompt, StoreError> {
        self.write(|d| {
            if !d.prompts.contains_key(id) {
                return Err(StoreError::NotFound);
            }
            if let Some(b) = meta.binding.as_deref()
                && let Some(other) = d.holder(b, id)
            {
                return Err(StoreError::Conflict(format!("{b} is bound to {other}")));
            }
            let p = d.prompt_mut(id)?;
            p.title = meta.title.clone();
            p.description = meta.description.clone();
            p.tags = meta.tags.clone();
            p.binding = meta.binding.clone();
            p.updated_ms = now_ms;
            let prompt = p.prompt();
            d.log(id, Action::Edit, None, actor, now_ms);
            Ok(prompt)
        })
    }

    fn set_active(
        &self,
        id: &str,
        version: Option<u32>,
        actor: &str,
        now_ms: u64,
    ) -> Result<Prompt, StoreError> {
        self.write(|d| {
            let p = d.prompt_mut(id)?;
            if let Some(v) = version
                && !p.versions.iter().any(|x| x.version == v)
            {
                return Err(StoreError::Invalid(format!("{id} has no version {v}")));
            }
            p.active_version = version;
            p.updated_ms = now_ms;
            let prompt = p.prompt();
            let action = if version.is_some() {
                Action::Publish
            } else {
                Action::Unpublish
            };
            d.log(id, action, version, actor, now_ms);
            Ok(prompt)
        })
    }

    fn set_archived(
        &self,
        id: &str,
        archived: bool,
        actor: &str,
        now_ms: u64,
    ) -> Result<Prompt, StoreError> {
        self.write(|d| {
            let binding = d.prompt_mut(id)?.binding.clone();
            if let Some(b) = binding.as_deref()
                && let Some(other) = d.holder(b, id)
            {
                return Err(StoreError::Conflict(format!("{b} is bound to {other}")));
            }
            let p = d.prompt_mut(id)?;
            p.archived_ms = if archived {
                p.archived_ms.or(Some(now_ms))
            } else {
                None
            };
            p.updated_ms = now_ms;
            let prompt = p.prompt();
            let action = if archived {
                Action::Archive
            } else {
                Action::Restore
            };
            d.log(id, action, None, actor, now_ms);
            Ok(prompt)
        })
    }

    fn events(&self, id: &str, limit: u32) -> Result<Vec<Event>, String> {
        self.read(|d| {
            d.events
                .iter()
                .rev()
                .filter(|e| e.prompt_id == id)
                .take(limit as usize)
                .map(|e| {
                    let action = Action::parse(&e.action).ok_or_else(|| {
                        format!("unknown action {} in the history of {id}", e.action)
                    })?;
                    Ok(Event {
                        id: e.id,
                        prompt_id: e.prompt_id.clone(),
                        action,
                        version: e.version,
                        actor: e.actor.clone(),
                        at_ms: e.at_ms,
                    })
                })
                .collect::<Result<Vec<Event>, String>>()
        })
    }

    fn bound(&self, binding: &str) -> Result<Option<(Prompt, String)>, String> {
        Ok(self.read(|d| {
            let p = d
                .prompts
                .values()
                .find(|p| p.live() && p.binding.as_deref() == Some(binding))?;
            let v = p.active_version?;
            let body = p.versions.iter().find(|x| x.version == v)?.body.clone();
            Some((p.prompt(), body))
        }))
    }
}
