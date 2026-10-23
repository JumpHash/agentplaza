//! `SkillStore` in `plaza.db`, as typed tables (`derive(apz_store::Table)` on the records):
//! - `skill` (keyed by id; its tags and bindings are the lists `skill_tags` and `skill_bindings`, its source the
//!   `source_*` columns), the editable record and the live version;
//! - `skill_version` (keyed by skill and version), immutable once written; its files are `skill_file` (keyed by skill,
//!   version and path) and its check's findings are `skill_finding` (keyed by skill, version and seq), one row each;
//! - `skill_event` (keyed by id), the history, appended only.
//!
//! The store keeps nothing in memory: each call reads the database, and each write is one closure of the file's
//! writer, so it is one transaction, and the call returns once it has committed. A refused write (a conflict, a
//! missing version) is decided before any row changes, so its transaction holds nothing.

use std::sync::Arc;

use apz_skill::Finding;
use apz_store::rusqlite::{Connection, OptionalExtension, Params, params};
use apz_store::table::{Changes, Loaded, Row, Value, apply};
use apz_store::{Db, RowSet, StoreError as DbError, Table, TableDef};

use super::{
    Action, Event, NewSkill, NewVersion, Skill, SkillFile, SkillStore, Source, StoreError, Version,
    VersionMeta,
};
use crate::store::sql;

/// The skill library in `plaza.db`. It holds no records of its own.
pub struct DbSkills {
    db: Arc<Db>,
    /// A library made by `for_tests` keeps its store, and so its folder, alive with it.
    #[cfg(test)]
    _keep: Option<(tempfile::TempDir, apz_store::Store)>,
}

/// What a write decided inside its transaction. The checks run before any row changes, so a refusal commits nothing.
enum Outcome<T> {
    Done(T),
    NotFound,
    Conflict(String),
    Invalid(String),
}

/// A version as its row of `skill_version` holds it: keyed by its skill and its number. Its files and findings are
/// rows of their own tables.
#[derive(Debug, Clone, PartialEq, Table)]
#[table(name = "skill_version")]
struct VersionRecord {
    #[table(key)]
    skill_id: String,
    #[table(key)]
    version: u32,
    note: String,
    author: String,
    created_ms: u64,
    score: u8,
    errors: u32,
    warnings: u32,
    infos: u32,
    rules_version: u32,
    /// BLAKE3 of SKILL.md, lower-case hex.
    hash: String,
    file_count: u32,
    total_bytes: u64,
    source_commit: Option<String>,
    skill_md: String,
}

/// One file of a version as its row of `skill_file`. The bytes are in the object store, under `hash`.
#[derive(Debug, Clone, PartialEq, Table)]
#[table(name = "skill_file")]
struct FileRecord {
    #[table(key)]
    skill_id: String,
    #[table(key)]
    version: u32,
    #[table(key)]
    path: String,
    /// BLAKE3 of the content, lower-case hex.
    hash: String,
    size: u64,
    executable: bool,
}

/// One check finding of a version as its row of `skill_finding`, keyed by its place in the order the check made them.
#[derive(Debug, Clone, PartialEq, Table)]
#[table(name = "skill_finding")]
struct FindingRecord {
    #[table(key)]
    skill_id: String,
    #[table(key)]
    version: u32,
    #[table(key)]
    seq: u32,
    #[table(flatten)]
    finding: Finding,
}

/// The table of skills (`skill`, with its lists `skill_tags` and `skill_bindings`). Written through.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(Skill)
}

/// The table of versions (`skill_version`). Written through; a row is never written again.
pub fn version_def() -> &'static TableDef {
    apz_store::root_def!(VersionRecord)
}

/// The table of version files (`skill_file`).
pub fn file_def() -> &'static TableDef {
    apz_store::root_def!(FileRecord)
}

/// The table of check findings (`skill_finding`).
pub fn finding_def() -> &'static TableDef {
    apz_store::root_def!(FindingRecord)
}

/// The table of history rows (`skill_event`).
pub fn event_def() -> &'static TableDef {
    apz_store::root_def!(Event)
}

impl DbSkills {
    /// The library in `db`, the hub's `plaza.db`.
    pub fn new(db: Arc<Db>) -> DbSkills {
        DbSkills {
            db,
            #[cfg(test)]
            _keep: None,
        }
    }

    /// A library in a fresh store folder, kept alive with the library (the folder goes when the library drops).
    #[cfg(test)]
    pub fn for_tests() -> DbSkills {
        let dir = tempfile::Builder::new()
            .prefix("apid-skills")
            .tempdir()
            .expect("a temporary folder");
        let store = crate::store::open(dir.path()).expect("the store opens");
        DbSkills {
            db: Arc::clone(store.db(crate::store::PLAZA)),
            _keep: Some((dir, store)),
        }
    }

    fn read<T>(&self, look: impl FnOnce(&Connection) -> Result<T, DbError>) -> Result<T, String> {
        self.db.read(look).map_err(|e| e.to_string())
    }

    /// Runs one write as one transaction and waits for its commit; only `Done` is a success.
    fn write<T: Send + 'static>(
        &self,
        op: impl FnOnce(&Connection) -> Result<Outcome<T>, DbError> + Send + 'static,
    ) -> Result<T, StoreError> {
        let written = self
            .db
            .write(op)
            .map_err(|e| StoreError::Failed(e.to_string()))?;
        match written.value {
            Outcome::Done(value) => Ok(value),
            Outcome::NotFound => Err(StoreError::NotFound),
            Outcome::Conflict(msg) => Err(StoreError::Conflict(msg)),
            Outcome::Invalid(msg) => Err(StoreError::Invalid(msg)),
        }
    }
}

impl SkillStore for DbSkills {
    fn list(&self, archived: bool) -> Result<Vec<Skill>, String> {
        self.read(|conn| skills_of(conn, sql::SKILL_LIST, params![archived]))
    }

    fn get(&self, id: &str) -> Result<Option<Skill>, String> {
        self.read(|conn| skill_of(conn, id))
    }

    fn versions(&self, id: &str) -> Result<Vec<VersionMeta>, String> {
        self.read(|conn| {
            let mut stmt = conn.prepare_cached(sql::SKILL_VERSIONS)?;
            let metas = stmt
                .query_map(params![id], meta_of_row)?
                .collect::<Result<Vec<VersionMeta>, _>>()?;
            Ok(metas)
        })
    }

    fn version(&self, id: &str, version: u32) -> Result<Option<Version>, String> {
        self.read(|conn| version_of(conn, id, version))
    }

    fn create(
        &self,
        new: &NewSkill,
        first: &NewVersion,
        actor: &str,
        now_ms: u64,
    ) -> Result<Skill, StoreError> {
        let (new, first, actor) = (new.clone(), first.clone(), actor.to_string());
        self.write(move |conn| {
            if skill_of(conn, &new.id)?.is_some() {
                return Ok(Outcome::Conflict(format!(
                    "a skill with id {} already exists",
                    new.id
                )));
            }
            let name = name_of(&first, &new.id);
            if let Some(holder) = name_holder(conn, &name, &new.id)? {
                return Ok(Outcome::Conflict(format!(
                    "name {name} is held by {holder}"
                )));
            }
            let skill = Skill {
                id: new.id.clone(),
                name,
                title: new.title.clone(),
                description: first.report.description.clone().unwrap_or_default(),
                tags: new.tags.clone(),
                bindings: normalised(&new.bindings),
                active_version: None,
                latest_version: 1,
                source: new.source.clone(),
                created_ms: now_ms,
                updated_ms: now_ms,
                archived_ms: None,
            };
            save_skill(conn, &skill)?;
            save_version(conn, &new.id, 1, &first, &actor, now_ms)?;
            record_event(conn, &new.id, Action::Create, None, &actor, now_ms)?;
            record_event(conn, &new.id, Action::Version, Some(1), &actor, now_ms)?;
            Ok(Outcome::Done(skill))
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
        let (id, next, actor) = (id.to_string(), next.clone(), actor.to_string());
        self.write(move |conn| {
            let Some(mut skill) = skill_of(conn, &id)? else {
                return Ok(Outcome::NotFound);
            };
            if skill.latest_version != base {
                return Ok(Outcome::Conflict(format!(
                    "{id} is at version {}, not {base}",
                    skill.latest_version
                )));
            }
            let name = name_of(&next, &id);
            if let Some(holder) = name_holder(conn, &name, &id)? {
                return Ok(Outcome::Conflict(format!(
                    "name {name} is held by {holder}"
                )));
            }
            let version = base + 1;
            let record = save_version(conn, &id, version, &next, &actor, now_ms)?;
            skill.latest_version = version;
            skill.name = name;
            skill.description = next.report.description.clone().unwrap_or_default();
            if let (Source::Github { commit, .. }, Some(c)) =
                (&mut skill.source, &next.source_commit)
            {
                *commit = c.clone();
            }
            skill.updated_ms = now_ms;
            save_skill(conn, &skill)?;
            record_event(conn, &id, Action::Version, Some(version), &actor, now_ms)?;
            Ok(Outcome::Done(record.meta()))
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
        let (id, title, tags, actor) = (
            id.to_string(),
            title.to_string(),
            tags.to_vec(),
            actor.to_string(),
        );
        self.write(move |conn| {
            let Some(mut skill) = skill_of(conn, &id)? else {
                return Ok(Outcome::NotFound);
            };
            skill.title = title;
            skill.tags = tags;
            skill.updated_ms = now_ms;
            save_skill(conn, &skill)?;
            record_event(conn, &id, Action::Edit, None, &actor, now_ms)?;
            Ok(Outcome::Done(skill))
        })
    }

    fn set_bindings(
        &self,
        id: &str,
        bindings: &[String],
        actor: &str,
        now_ms: u64,
    ) -> Result<Skill, StoreError> {
        let (id, bindings, actor) = (id.to_string(), normalised(bindings), actor.to_string());
        self.write(move |conn| {
            let Some(mut skill) = skill_of(conn, &id)? else {
                return Ok(Outcome::NotFound);
            };
            skill.bindings = bindings;
            skill.updated_ms = now_ms;
            save_skill(conn, &skill)?;
            record_event(conn, &id, Action::Bind, None, &actor, now_ms)?;
            Ok(Outcome::Done(skill))
        })
    }

    fn set_active(
        &self,
        id: &str,
        version: Option<u32>,
        actor: &str,
        now_ms: u64,
    ) -> Result<Skill, StoreError> {
        let (id, actor) = (id.to_string(), actor.to_string());
        self.write(move |conn| {
            let Some(mut skill) = skill_of(conn, &id)? else {
                return Ok(Outcome::NotFound);
            };
            let action = match version {
                Some(v) => {
                    if version_meta(conn, &id, v)?.is_none() {
                        return Ok(Outcome::Invalid(format!("{id} has no version {v}")));
                    }
                    Action::Publish
                }
                None => Action::Unpublish,
            };
            skill.active_version = version;
            skill.updated_ms = now_ms;
            save_skill(conn, &skill)?;
            record_event(conn, &id, action, version, &actor, now_ms)?;
            Ok(Outcome::Done(skill))
        })
    }

    fn set_archived(
        &self,
        id: &str,
        archived: bool,
        actor: &str,
        now_ms: u64,
    ) -> Result<Skill, StoreError> {
        let (id, actor) = (id.to_string(), actor.to_string());
        self.write(move |conn| {
            let Some(mut skill) = skill_of(conn, &id)? else {
                return Ok(Outcome::NotFound);
            };
            if !archived && let Some(holder) = name_holder(conn, &skill.name, &id)? {
                return Ok(Outcome::Conflict(format!(
                    "name {} is held by {holder}",
                    skill.name
                )));
            }
            skill.archived_ms = if archived {
                skill.archived_ms.or(Some(now_ms))
            } else {
                None
            };
            skill.updated_ms = now_ms;
            save_skill(conn, &skill)?;
            let action = if archived {
                Action::Archive
            } else {
                Action::Restore
            };
            record_event(conn, &id, action, None, &actor, now_ms)?;
            Ok(Outcome::Done(skill))
        })
    }

    fn events(&self, id: &str, limit: u32) -> Result<Vec<Event>, String> {
        self.read(|conn| {
            read_rows(
                conn,
                sql::SKILL_EVENTS,
                event_def(),
                params![id, i64::from(limit)],
            )
        })
    }

    fn bound(&self, binding: &str) -> Result<Vec<(Skill, VersionMeta)>, String> {
        self.read(|conn| {
            let mut out = Vec::new();
            for skill in skills_of(conn, sql::SKILL_BOUND, params![binding])? {
                let Some(live) = skill.active_version else {
                    continue;
                };
                if let Some(meta) = version_meta(conn, &skill.id, live)? {
                    out.push((skill, meta));
                }
            }
            Ok(out)
        })
    }
}

impl VersionRecord {
    fn new(
        skill_id: &str,
        version: u32,
        next: &NewVersion,
        actor: &str,
        now_ms: u64,
    ) -> VersionRecord {
        let report = &next.report;
        VersionRecord {
            skill_id: skill_id.to_string(),
            version,
            note: next.note.clone(),
            author: actor.to_string(),
            created_ms: now_ms,
            score: report.score,
            errors: report.errors,
            warnings: report.warnings,
            infos: report.infos,
            rules_version: report.rules_version,
            hash: next.hash.clone(),
            file_count: next.files.len() as u32,
            total_bytes: next.files.iter().map(|f| f.size).sum(),
            source_commit: next.source_commit.clone(),
            skill_md: next.skill_md.clone(),
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
            file_count: self.file_count,
            total_bytes: self.total_bytes,
            source_commit: self.source_commit.clone(),
        }
    }
}

/// Takes the rows `query` selects as `T`. The query selects every column of `def`'s table in the table's order, as
/// `SELECT *` does. A root's lists are not read here: the caller fills them.
fn read_rows<T: Table>(
    conn: &Connection,
    query: &str,
    def: &TableDef,
    params: impl Params,
) -> Result<Vec<T>, DbError> {
    let width = def.key.len() + def.columns.len();
    let mut stmt = conn.prepare_cached(query)?;
    let mut rows = stmt.query(params)?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let mut values = Vec::with_capacity(width);
        for i in 0..width {
            values.push(r.get::<_, Value>(i)?);
        }
        let columns = values.split_off(def.key.len());
        let row = Row {
            key: values,
            columns,
        };
        out.push(T::take(def, &row, &Loaded::default())?);
    }
    Ok(out)
}

/// One row of `skill_versions.sql` or `skill_version_meta.sql`: a version's metadata, its SKILL.md left out.
fn meta_of_row(r: &apz_store::rusqlite::Row<'_>) -> apz_store::rusqlite::Result<VersionMeta> {
    Ok(VersionMeta {
        version: r.get(0)?,
        note: r.get(1)?,
        author: r.get(2)?,
        created_ms: r.get(3)?,
        score: r.get(4)?,
        errors: r.get(5)?,
        warnings: r.get(6)?,
        infos: r.get(7)?,
        rules_version: r.get(8)?,
        hash: r.get(9)?,
        file_count: r.get(10)?,
        total_bytes: r.get(11)?,
        source_commit: r.get(12)?,
    })
}

/// The skills `query` selects, each with its tags and bindings in the order they were stored.
fn skills_of(conn: &Connection, query: &str, params: impl Params) -> Result<Vec<Skill>, DbError> {
    let mut skills: Vec<Skill> = read_rows(conn, query, def(), params)?;
    for skill in &mut skills {
        skill.tags = list_of(conn, sql::SKILL_TAGS, &skill.id)?;
        skill.bindings = list_of(conn, sql::SKILL_BINDINGS, &skill.id)?;
    }
    Ok(skills)
}

/// The skill with `id`, if there is one.
fn skill_of(conn: &Connection, id: &str) -> Result<Option<Skill>, DbError> {
    Ok(skills_of(conn, sql::SKILL_GET, params![id])?
        .into_iter()
        .next())
}

/// The values of one of a skill's lists (`skill_tags.sql` or `skill_bindings.sql`), in their order.
fn list_of(conn: &Connection, query: &str, id: &str) -> Result<Vec<String>, DbError> {
    let mut stmt = conn.prepare_cached(query)?;
    let values = stmt
        .query_map(params![id], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(values)
}

/// Writes a skill's row and its lists. The lists are written whole: their rows are cleared first.
fn save_skill(conn: &Connection, skill: &Skill) -> Result<(), DbError> {
    let place: i64 = conn
        .prepare_cached(sql::SKILL_POS)?
        .query_row(params![skill.id], |r| r.get(0))?;
    conn.prepare_cached(sql::SKILL_TAGS_CLEAR)?
        .execute(params![skill.id])?;
    conn.prepare_cached(sql::SKILL_BINDINGS_CLEAR)?
        .execute(params![skill.id])?;
    save(conn, def(), skill, u64::try_from(place).unwrap_or(0))
}

/// Writes a version: its row, then one row for each of its files and for each of its check's findings. A version is
/// immutable, so nothing writes its rows again.
fn save_version(
    conn: &Connection,
    id: &str,
    version: u32,
    next: &NewVersion,
    actor: &str,
    now_ms: u64,
) -> Result<VersionRecord, DbError> {
    let record = VersionRecord::new(id, version, next, actor, now_ms);
    save(conn, version_def(), &record, u64::from(version))?;
    for (pos, file) in next.files.iter().enumerate() {
        let row = FileRecord {
            skill_id: id.to_string(),
            version,
            path: file.path.clone(),
            hash: file.hash.clone(),
            size: file.size,
            executable: file.executable,
        };
        save(conn, file_def(), &row, pos as u64)?;
    }
    for (seq, finding) in next.report.findings.iter().enumerate() {
        let row = FindingRecord {
            skill_id: id.to_string(),
            version,
            seq: seq as u32,
            finding: finding.clone(),
        };
        save(conn, finding_def(), &row, seq as u64)?;
    }
    Ok(record)
}

/// Writes a record and its child rows as upserts: its root row at `pos` (the table's place for the row), then every
/// child table's rows.
fn save<T: Table>(conn: &Connection, def: &TableDef, value: &T, pos: u64) -> Result<(), DbError> {
    let mut rows = RowSet::new();
    value.put(
        def,
        vec![Value::Integer(i64::try_from(pos).unwrap_or(i64::MAX))],
        &mut rows,
    )?;
    let mut changes = Changes::default();
    for table in def.walk() {
        for row in rows.tables.remove(&table.name).unwrap_or_default() {
            changes.upserts.push((table.name.clone(), row));
        }
    }
    apply(conn, def, &changes)
}

/// The metadata of version `version` of skill `id`, if there is such a version.
fn version_meta(conn: &Connection, id: &str, version: u32) -> Result<Option<VersionMeta>, DbError> {
    Ok(conn
        .prepare_cached(sql::SKILL_VERSION_META)?
        .query_row(params![id, i64::from(version)], meta_of_row)
        .optional()?)
}

/// Version `version` of skill `id` with its files and findings, if there is such a version.
fn version_of(conn: &Connection, id: &str, version: u32) -> Result<Option<Version>, DbError> {
    let found: Vec<VersionRecord> = read_rows(
        conn,
        sql::SKILL_VERSION_GET,
        version_def(),
        params![id, i64::from(version)],
    )?;
    let Some(record) = found.into_iter().next() else {
        return Ok(None);
    };
    let files: Vec<FileRecord> = read_rows(
        conn,
        sql::SKILL_FILES,
        file_def(),
        params![id, i64::from(version)],
    )?;
    let findings: Vec<FindingRecord> = read_rows(
        conn,
        sql::SKILL_FINDINGS,
        finding_def(),
        params![id, i64::from(version)],
    )?;
    Ok(Some(Version {
        meta: record.meta(),
        skill_md: record.skill_md,
        files: files
            .into_iter()
            .map(|f| SkillFile {
                path: f.path,
                hash: f.hash,
                size: f.size,
                executable: f.executable,
            })
            .collect(),
        findings: findings.into_iter().map(|f| f.finding).collect(),
    }))
}

/// Appends one history row. Its id is the next after the newest row of any skill, so the history is one stream.
fn record_event(
    conn: &Connection,
    skill_id: &str,
    action: Action,
    version: Option<u32>,
    actor: &str,
    at_ms: u64,
) -> Result<(), DbError> {
    let last: i64 = conn
        .prepare_cached(sql::SKILL_EVENT_LAST_SEQ)?
        .query_row([], |r| r.get(0))?;
    let id = u64::try_from(last).unwrap_or(0) + 1;
    let event = Event {
        id,
        skill_id: skill_id.to_string(),
        action,
        version,
        actor: actor.to_string(),
        at_ms,
    };
    save(conn, event_def(), &event, id)
}

/// The id of another live skill holding `name`, if any.
fn name_holder(conn: &Connection, name: &str, id: &str) -> Result<Option<String>, DbError> {
    Ok(conn
        .prepare_cached(sql::SKILL_NAME_HOLDER)?
        .query_row(params![name, id], |r| r.get(0))
        .optional()?)
}

/// A version's name is its frontmatter name, or the skill id when it has none.
fn name_of(next: &NewVersion, id: &str) -> String {
    next.report.name.clone().unwrap_or_else(|| id.to_string())
}

/// Sorted and deduplicated bindings.
fn normalised(bindings: &[String]) -> Vec<String> {
    let mut out = bindings.to_vec();
    out.sort();
    out.dedup();
    out
}
