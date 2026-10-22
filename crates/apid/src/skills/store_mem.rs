//! `SkillStore` without Postgres: everything in memory, persisted to `<state_dir>/skills.json` (written to a
//! temporary file, fsynced and renamed over the old one after every write). The same rules as `store_db`.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use apz_skill::{Finding, Report};
use serde::{Deserialize, Serialize};

use super::{
    Action, Event, NewSkill, NewVersion, Skill, SkillFile, SkillStore, Source, StoreError, Version,
    VersionMeta,
};

pub struct MemorySkills {
    /// `None` when nothing is persisted.
    path: Option<PathBuf>,
    inner: Mutex<Data>,
}

/// What the file holds.
#[derive(Clone, Default, Serialize, Deserialize)]
struct Data {
    /// By id.
    skills: BTreeMap<String, Stored>,
    /// Every event of every skill, oldest first.
    events: Vec<StoredEvent>,
    /// The id of the last event written; the next one is this plus one.
    last_event: u64,
}

#[derive(Clone, Serialize, Deserialize)]
struct Stored {
    id: String,
    name: String,
    title: String,
    description: String,
    tags: Vec<String>,
    /// Sorted and deduplicated.
    bindings: Vec<String>,
    source: StoredSource,
    active_version: Option<u32>,
    created_ms: u64,
    updated_ms: u64,
    archived_ms: Option<u64>,
    /// Oldest first: version `n` is at index `n - 1`.
    versions: Vec<StoredVersion>,
}

#[derive(Clone, Serialize, Deserialize)]
enum StoredSource {
    Built,
    Upload,
    Github {
        repo: String,
        git_ref: Option<String>,
        path: String,
        commit: String,
    },
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
    hash: String,
    skill_md: String,
    files: Vec<StoredFile>,
    findings: Vec<Finding>,
    source_commit: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredFile {
    path: String,
    hash: String,
    size: u64,
    executable: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredEvent {
    id: u64,
    skill_id: String,
    /// `Action::as_str`.
    action: String,
    version: Option<u32>,
    actor: String,
    at_ms: u64,
}

/// The name and description a skill takes from its latest version's frontmatter (the id and "" when it did not parse).
fn frontmatter(report: &Report, id: &str) -> (String, String) {
    (
        report.name.clone().unwrap_or_else(|| id.to_string()),
        report.description.clone().unwrap_or_default(),
    )
}

/// Sorted and deduplicated bindings.
fn normalised(bindings: &[String]) -> Vec<String> {
    let mut out = bindings.to_vec();
    out.sort();
    out.dedup();
    out
}

impl Data {
    /// The live skill, other than `except`, whose name is `name`.
    fn name_holder(&self, name: &str, except: &str) -> Option<&str> {
        self.skills
            .values()
            .find(|s| s.live() && s.id != except && s.name == name)
            .map(|s| s.id.as_str())
    }

    fn skill_mut(&mut self, id: &str) -> Result<&mut Stored, StoreError> {
        self.skills.get_mut(id).ok_or(StoreError::NotFound)
    }

    fn log(
        &mut self,
        skill_id: &str,
        action: Action,
        version: Option<u32>,
        actor: &str,
        at_ms: u64,
    ) {
        self.last_event += 1;
        self.events.push(StoredEvent {
            id: self.last_event,
            skill_id: skill_id.to_string(),
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

    fn skill(&self) -> Skill {
        Skill {
            id: self.id.clone(),
            name: self.name.clone(),
            title: self.title.clone(),
            description: self.description.clone(),
            tags: self.tags.clone(),
            bindings: self.bindings.clone(),
            active_version: self.active_version,
            latest_version: self.versions.len() as u32,
            source: self.source.to_source(),
            created_ms: self.created_ms,
            updated_ms: self.updated_ms,
            archived_ms: self.archived_ms,
        }
    }

    fn version(&self, version: u32) -> Option<&StoredVersion> {
        self.versions.iter().find(|v| v.version == version)
    }
}

impl StoredSource {
    fn from_source(source: &Source) -> StoredSource {
        match source {
            Source::Built => StoredSource::Built,
            Source::Upload => StoredSource::Upload,
            Source::Github {
                repo,
                git_ref,
                path,
                commit,
            } => StoredSource::Github {
                repo: repo.clone(),
                git_ref: git_ref.clone(),
                path: path.clone(),
                commit: commit.clone(),
            },
        }
    }

    fn to_source(&self) -> Source {
        match self {
            StoredSource::Built => Source::Built,
            StoredSource::Upload => Source::Upload,
            StoredSource::Github {
                repo,
                git_ref,
                path,
                commit,
            } => Source::Github {
                repo: repo.clone(),
                git_ref: git_ref.clone(),
                path: path.clone(),
                commit: commit.clone(),
            },
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
            hash: next.hash.clone(),
            skill_md: next.skill_md.clone(),
            files: next
                .files
                .iter()
                .map(|f| StoredFile {
                    path: f.path.clone(),
                    hash: f.hash.clone(),
                    size: f.size,
                    executable: f.executable,
                })
                .collect(),
            findings: next.report.findings.clone(),
            source_commit: next.source_commit.clone(),
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
            hash: self.hash.clone(),
            file_count: self.files.len() as u32,
            total_bytes: self.files.iter().map(|f| f.size).sum(),
            source_commit: self.source_commit.clone(),
        }
    }

    fn full(&self) -> Version {
        Version {
            meta: self.meta(),
            skill_md: self.skill_md.clone(),
            files: self
                .files
                .iter()
                .map(|f| SkillFile {
                    path: f.path.clone(),
                    hash: f.hash.clone(),
                    size: f.size,
                    executable: f.executable,
                })
                .collect(),
            findings: self.findings.clone(),
        }
    }
}

impl MemorySkills {
    /// Not persisted (tests).
    #[cfg(test)]
    pub fn in_memory() -> MemorySkills {
        MemorySkills {
            path: None,
            inner: Mutex::new(Data::default()),
        }
    }

    /// Loads `<state_dir>/skills.json` if it exists, and writes it after every change.
    pub fn open(state_dir: &Path) -> MemorySkills {
        let path = state_dir.join("skills.json");
        let data = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Data>(&bytes).unwrap_or_else(|err| {
                eprintln!(
                    "apid: skill library: {} does not parse ({err}); starting empty, the file is kept as skills.json.bad",
                    path.display()
                );
                if let Err(e) = std::fs::rename(&path, state_dir.join("skills.json.bad")) {
                    eprintln!("apid: skill library: keeping the unreadable file failed: {e}");
                }
                Data::default()
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Data::default(),
            Err(err) => {
                // The file is there but unreadable: never overwrite it. This run keeps the library in memory only.
                eprintln!(
                    "apid: skill library: reading {} failed ({err}); the library stays in memory this run and the file is left as it is",
                    path.display()
                );
                return MemorySkills { path: None, inner: Mutex::new(Data::default()) };
            }
        };
        MemorySkills {
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
            .map_err(|e| StoreError::Failed(format!("encoding the skill library: {e}")))?;
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

impl SkillStore for MemorySkills {
    fn list(&self, archived: bool) -> Result<Vec<Skill>, String> {
        Ok(self.read(|d| {
            d.skills
                .values()
                .filter(|s| archived || s.live())
                .map(Stored::skill)
                .collect()
        }))
    }

    fn get(&self, id: &str) -> Result<Option<Skill>, String> {
        Ok(self.read(|d| d.skills.get(id).map(Stored::skill)))
    }

    fn versions(&self, id: &str) -> Result<Vec<VersionMeta>, String> {
        Ok(self.read(|d| {
            d.skills
                .get(id)
                .map(|s| s.versions.iter().map(StoredVersion::meta).collect())
                .unwrap_or_default()
        }))
    }

    fn version(&self, id: &str, version: u32) -> Result<Option<Version>, String> {
        Ok(self.read(|d| d.skills.get(id)?.version(version).map(StoredVersion::full)))
    }

    fn create(
        &self,
        new: &NewSkill,
        first: &NewVersion,
        actor: &str,
        now_ms: u64,
    ) -> Result<Skill, StoreError> {
        self.write(|d| {
            if d.skills.contains_key(&new.id) {
                return Err(StoreError::Conflict(format!(
                    "a skill named {} exists",
                    new.id
                )));
            }
            let (name, description) = frontmatter(&first.report, &new.id);
            if let Some(other) = d.name_holder(&name, &new.id) {
                return Err(StoreError::Conflict(format!(
                    "{name} is the name of {other}"
                )));
            }
            let stored = Stored {
                id: new.id.clone(),
                name,
                title: new.title.clone(),
                description,
                tags: new.tags.clone(),
                bindings: normalised(&new.bindings),
                source: StoredSource::from_source(&new.source),
                active_version: None,
                created_ms: now_ms,
                updated_ms: now_ms,
                archived_ms: None,
                versions: vec![StoredVersion::new(1, first, actor, now_ms)],
            };
            let skill = stored.skill();
            d.skills.insert(new.id.clone(), stored);
            d.log(&new.id, Action::Create, None, actor, now_ms);
            d.log(&new.id, Action::Version, Some(1), actor, now_ms);
            Ok(skill)
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
            let latest = d.skills.get(id).ok_or(StoreError::NotFound)?.versions.len() as u32;
            if base != latest {
                return Err(StoreError::Conflict(format!(
                    "{id} is at version {latest}, not {base}"
                )));
            }
            let (name, description) = frontmatter(&next.report, id);
            if let Some(other) = d.name_holder(&name, id) {
                return Err(StoreError::Conflict(format!(
                    "{name} is the name of {other}"
                )));
            }
            let v = StoredVersion::new(latest + 1, next, actor, now_ms);
            let meta = v.meta();
            let s = d.skill_mut(id)?;
            s.versions.push(v);
            s.name = name;
            s.description = description;
            s.updated_ms = now_ms;
            if let (StoredSource::Github { commit, .. }, Some(c)) =
                (&mut s.source, &next.source_commit)
            {
                *commit = c.clone();
            }
            d.log(id, Action::Version, Some(latest + 1), actor, now_ms);
            Ok(meta)
        })
    }

    fn set_meta(
        &self,
        id: &str,
        title: &str,
        tags: &[String],
        actor: &str,
        now_ms: u64,
    ) -> Result<Skill, StoreError> {
        self.write(|d| {
            let s = d.skill_mut(id)?;
            s.title = title.to_string();
            s.tags = tags.to_vec();
            s.updated_ms = now_ms;
            let skill = s.skill();
            d.log(id, Action::Edit, None, actor, now_ms);
            Ok(skill)
        })
    }

    fn set_bindings(
        &self,
        id: &str,
        bindings: &[String],
        actor: &str,
        now_ms: u64,
    ) -> Result<Skill, StoreError> {
        self.write(|d| {
            let s = d.skill_mut(id)?;
            s.bindings = normalised(bindings);
            s.updated_ms = now_ms;
            let skill = s.skill();
            d.log(id, Action::Bind, None, actor, now_ms);
            Ok(skill)
        })
    }

    fn set_active(
        &self,
        id: &str,
        version: Option<u32>,
        actor: &str,
        now_ms: u64,
    ) -> Result<Skill, StoreError> {
        self.write(|d| {
            let s = d.skill_mut(id)?;
            if let Some(v) = version
                && s.version(v).is_none()
            {
                return Err(StoreError::Invalid(format!("{id} has no version {v}")));
            }
            s.active_version = version;
            s.updated_ms = now_ms;
            let skill = s.skill();
            let action = if version.is_some() {
                Action::Publish
            } else {
                Action::Unpublish
            };
            d.log(id, action, version, actor, now_ms);
            Ok(skill)
        })
    }

    fn set_archived(
        &self,
        id: &str,
        archived: bool,
        actor: &str,
        now_ms: u64,
    ) -> Result<Skill, StoreError> {
        self.write(|d| {
            let name = d.skill_mut(id)?.name.clone();
            if !archived && let Some(other) = d.name_holder(&name, id) {
                return Err(StoreError::Conflict(format!(
                    "{name} is the name of {other}"
                )));
            }
            let s = d.skill_mut(id)?;
            s.archived_ms = if archived {
                s.archived_ms.or(Some(now_ms))
            } else {
                None
            };
            s.updated_ms = now_ms;
            let skill = s.skill();
            let action = if archived {
                Action::Archive
            } else {
                Action::Restore
            };
            d.log(id, action, None, actor, now_ms);
            Ok(skill)
        })
    }

    fn events(&self, id: &str, limit: u32) -> Result<Vec<Event>, String> {
        self.read(|d| {
            d.events
                .iter()
                .rev()
                .filter(|e| e.skill_id == id)
                .take(limit as usize)
                .map(|e| {
                    let action = Action::parse(&e.action).ok_or_else(|| {
                        format!("unknown action {} in the history of {id}", e.action)
                    })?;
                    Ok(Event {
                        id: e.id,
                        skill_id: e.skill_id.clone(),
                        action,
                        version: e.version,
                        actor: e.actor.clone(),
                        at_ms: e.at_ms,
                    })
                })
                .collect::<Result<Vec<Event>, String>>()
        })
    }

    fn bound(&self, binding: &str) -> Result<Vec<(Skill, VersionMeta)>, String> {
        Ok(self.read(|d| {
            let mut out: Vec<(Skill, VersionMeta)> = d
                .skills
                .values()
                .filter(|s| s.live() && s.bindings.iter().any(|b| b == binding))
                .filter_map(|s| {
                    let v = s.active_version?;
                    Some((s.skill(), s.version(v)?.meta()))
                })
                .collect();
            out.sort_by(|a, b| a.0.name.cmp(&b.0.name));
            out
        }))
    }
}
