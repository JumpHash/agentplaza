//! `PromptStore` in `plaza.db`, as typed tables (`derive(apz_store::Table)` on the records):
//! - `prompt` (keyed by id; its tags are the list `prompt_tags`), the editable record and the live version;
//! - `prompt_version` (keyed by prompt and version; its check's findings are the child `prompt_version_findings`),
//!   immutable once written: a new version is a new row;
//! - `prompt_event` (keyed by id, the history's place), appended only.
//!
//! The store keeps nothing in memory: each call reads the file, and each write is one closure of the file's writer,
//! so it is one transaction, and the call returns once it has committed. A refused write (a conflict, a missing
//! version) is decided before any row changes, so its transaction holds nothing.

use std::sync::Arc;

use apz_store::rusqlite::{Connection, OptionalExtension, Params, params};
use apz_store::table::{Changes, Loaded, Row, Value, apply};
use apz_store::{Db, RowSet, StoreError as DbError, Table, TableDef};

use super::{
    Action, Event, Meta, NewPrompt, NewVersion, Prompt, PromptStore, StoreError, Version,
    VersionMeta,
};
use crate::store::sql;

/// The prompt library in `plaza.db`. It holds no records of its own.
pub struct SqlitePrompts {
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

/// A version as its row of `prompt_version` holds it: keyed by its prompt and its number. `Version` and `VersionMeta`
/// are the shapes the API takes it in; the key is what they do not carry.
#[derive(Debug, Clone, PartialEq, Table)]
#[table(name = "prompt_version")]
struct VersionRecord {
    #[table(key)]
    prompt_id: String,
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
    /// sha256 of the body, lower-case hex (the text the check and the diff compare by).
    sha256: String,
    body: String,
    #[table(child)]
    findings: Vec<apz_prompt::Finding>,
}

impl VersionRecord {
    fn new(
        prompt_id: &str,
        version: u32,
        next: &NewVersion,
        author: &str,
        now_ms: u64,
    ) -> VersionRecord {
        let report = &next.report;
        VersionRecord {
            prompt_id: prompt_id.to_string(),
            version,
            note: next.note.clone(),
            author: author.to_string(),
            created_ms: now_ms,
            score: report.score,
            errors: report.errors,
            warnings: report.warnings,
            infos: report.infos,
            rules_version: report.rules_version,
            sha256: next.sha256.clone(),
            body: next.body.clone(),
            findings: report.findings.clone(),
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

    fn full(self) -> Version {
        Version {
            meta: VersionMeta {
                version: self.version,
                note: self.note,
                author: self.author,
                created_ms: self.created_ms,
                score: self.score,
                errors: self.errors,
                warnings: self.warnings,
                infos: self.infos,
                rules_version: self.rules_version,
                sha256: self.sha256,
            },
            body: self.body,
            findings: self.findings,
        }
    }
}

/// The table of prompts (`prompt`, and its tags `prompt_tags`). Written through: a change is durable when it returns.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(Prompt)
}

/// The table of versions (`prompt_version`, and its findings `prompt_version_findings`).
pub fn version_def() -> &'static TableDef {
    apz_store::root_def!(VersionRecord)
}

/// The table of history rows (`prompt_event`).
pub fn event_def() -> &'static TableDef {
    apz_store::root_def!(Event)
}

/// The findings table under a version: the one child of `prompt_version`.
fn findings_def() -> &'static TableDef {
    &version_def().children[0]
}

impl SqlitePrompts {
    /// The library in `db`, the hub's `plaza.db`.
    pub fn new(db: Arc<Db>) -> SqlitePrompts {
        SqlitePrompts {
            db,
            #[cfg(test)]
            _keep: None,
        }
    }

    /// A library in a fresh store folder, kept alive with the library (the folder goes when the library drops).
    #[cfg(test)]
    pub fn for_tests() -> SqlitePrompts {
        let dir = tempfile::Builder::new()
            .prefix("apid-prompts")
            .tempdir()
            .expect("a temporary folder");
        let store = crate::store::open_for_tests(dir.path()).expect("the store opens");
        SqlitePrompts {
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

impl PromptStore for SqlitePrompts {
    fn list(&self, archived: bool) -> Result<Vec<Prompt>, String> {
        self.read(|conn| prompts_of(conn, sql::PROMPT_LIST, params![archived]))
    }

    fn get(&self, id: &str) -> Result<Option<Prompt>, String> {
        self.read(|conn| prompt_of(conn, id))
    }

    fn versions(&self, id: &str) -> Result<Vec<VersionMeta>, String> {
        self.read(|conn| {
            let mut stmt = conn.prepare_cached(sql::PROMPT_VERSIONS)?;
            let metas = stmt
                .query_map(params![id], meta_of_row)?
                .collect::<Result<Vec<VersionMeta>, _>>()?;
            Ok(metas)
        })
    }

    fn version(&self, id: &str, version: u32) -> Result<Option<Version>, String> {
        self.read(|conn| Ok(version_of(conn, id, version)?.map(VersionRecord::full)))
    }

    fn create(
        &self,
        new: &NewPrompt,
        first: &NewVersion,
        actor: &str,
        now_ms: u64,
    ) -> Result<Prompt, StoreError> {
        let (new, first, actor) = (new.clone(), first.clone(), actor.to_string());
        self.write(move |conn| {
            if prompt_of(conn, &new.id)?.is_some() {
                return Ok(Outcome::Conflict(format!(
                    "a prompt named {} already exists",
                    new.id
                )));
            }
            if let Some(binding) = new.binding.as_deref()
                && let Some(holder) = binding_holder(conn, binding, &new.id)?
            {
                return Ok(Outcome::Conflict(format!(
                    "binding {binding} is held by {holder}"
                )));
            }
            let prompt = Prompt {
                id: new.id.clone(),
                kind: new.kind,
                title: new.title.clone(),
                description: new.description.clone(),
                tags: new.tags.clone(),
                binding: new.binding.clone(),
                active_version: None,
                latest_version: 1,
                created_ms: now_ms,
                updated_ms: now_ms,
                archived_ms: None,
            };
            save_prompt(conn, &prompt)?;
            save_version(
                conn,
                &VersionRecord::new(&new.id, 1, &first, &actor, now_ms),
            )?;
            record_event(conn, &new.id, Action::Create, None, &actor, now_ms)?;
            record_event(conn, &new.id, Action::Version, Some(1), &actor, now_ms)?;
            Ok(Outcome::Done(prompt))
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
            let Some(mut prompt) = prompt_of(conn, &id)? else {
                return Ok(Outcome::NotFound);
            };
            if prompt.latest_version != base {
                return Ok(Outcome::Conflict(format!(
                    "{id} is at version {}, not {base}",
                    prompt.latest_version
                )));
            }
            let version = base + 1;
            let record = VersionRecord::new(&id, version, &next, &actor, now_ms);
            let meta = record.meta();
            save_version(conn, &record)?;
            prompt.latest_version = version;
            prompt.updated_ms = now_ms;
            save_prompt(conn, &prompt)?;
            record_event(conn, &id, Action::Version, Some(version), &actor, now_ms)?;
            Ok(Outcome::Done(meta))
        })
    }

    fn set_meta(
        &self,
        id: &str,
        meta: &Meta,
        actor: &str,
        now_ms: u64,
    ) -> Result<Prompt, StoreError> {
        let (id, meta, actor) = (id.to_string(), meta.clone(), actor.to_string());
        self.write(move |conn| {
            let Some(mut prompt) = prompt_of(conn, &id)? else {
                return Ok(Outcome::NotFound);
            };
            if let Some(binding) = meta.binding.as_deref()
                && let Some(holder) = binding_holder(conn, binding, &id)?
            {
                return Ok(Outcome::Conflict(format!(
                    "binding {binding} is held by {holder}"
                )));
            }
            prompt.title = meta.title.clone();
            prompt.description = meta.description.clone();
            prompt.tags = meta.tags.clone();
            prompt.binding = meta.binding.clone();
            prompt.updated_ms = now_ms;
            save_prompt(conn, &prompt)?;
            record_event(conn, &id, Action::Edit, None, &actor, now_ms)?;
            Ok(Outcome::Done(prompt))
        })
    }

    fn set_active(
        &self,
        id: &str,
        version: Option<u32>,
        actor: &str,
        now_ms: u64,
    ) -> Result<Prompt, StoreError> {
        let (id, actor) = (id.to_string(), actor.to_string());
        self.write(move |conn| {
            let Some(mut prompt) = prompt_of(conn, &id)? else {
                return Ok(Outcome::NotFound);
            };
            let action = match version {
                Some(v) => {
                    if body_of(conn, &id, v)?.is_none() {
                        return Ok(Outcome::Invalid(format!("{id} has no version {v}")));
                    }
                    Action::Publish
                }
                None => Action::Unpublish,
            };
            prompt.active_version = version;
            prompt.updated_ms = now_ms;
            save_prompt(conn, &prompt)?;
            record_event(conn, &id, action, version, &actor, now_ms)?;
            Ok(Outcome::Done(prompt))
        })
    }

    fn set_archived(
        &self,
        id: &str,
        archived: bool,
        actor: &str,
        now_ms: u64,
    ) -> Result<Prompt, StoreError> {
        let (id, actor) = (id.to_string(), actor.to_string());
        self.write(move |conn| {
            let Some(mut prompt) = prompt_of(conn, &id)? else {
                return Ok(Outcome::NotFound);
            };
            if !archived
                && let Some(binding) = prompt.binding.clone()
                && let Some(holder) = binding_holder(conn, &binding, &id)?
            {
                return Ok(Outcome::Conflict(format!(
                    "binding {binding} is held by {holder}"
                )));
            }
            prompt.archived_ms = if archived {
                prompt.archived_ms.or(Some(now_ms))
            } else {
                None
            };
            prompt.updated_ms = now_ms;
            save_prompt(conn, &prompt)?;
            let action = if archived {
                Action::Archive
            } else {
                Action::Restore
            };
            record_event(conn, &id, action, None, &actor, now_ms)?;
            Ok(Outcome::Done(prompt))
        })
    }

    fn events(&self, id: &str, limit: u32) -> Result<Vec<Event>, String> {
        self.read(|conn| {
            read_rows(
                conn,
                sql::PROMPT_EVENTS,
                event_def(),
                params![id, i64::from(limit)],
            )
        })
    }

    fn bound(&self, binding: &str) -> Result<Option<(Prompt, String)>, String> {
        self.read(|conn| {
            let Some(prompt) = prompts_of(conn, sql::PROMPT_BOUND, params![binding])?
                .into_iter()
                .next()
            else {
                return Ok(None);
            };
            let Some(live) = prompt.active_version else {
                return Ok(None);
            };
            Ok(body_of(conn, &prompt.id, live)?.map(|body| (prompt, body)))
        })
    }
}

/// Takes the rows `query` selects as `T`. The query selects every column of `def`'s table in the table's order, as
/// `SELECT *` does. A root's children are not read here: the caller fills them.
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

/// One row of `prompt_versions.sql`: a version's metadata, its body left out.
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
        sha256: r.get(9)?,
    })
}

/// The prompts `query` selects, each with its tags in the order they were given.
fn prompts_of(conn: &Connection, query: &str, params: impl Params) -> Result<Vec<Prompt>, DbError> {
    let mut prompts: Vec<Prompt> = read_rows(conn, query, def(), params)?;
    for prompt in &mut prompts {
        prompt.tags = conn
            .prepare_cached(sql::PROMPT_TAGS)?
            .query_map(params![prompt.id], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<String>, _>>()?;
    }
    Ok(prompts)
}

/// The prompt with `id`, if there is one.
fn prompt_of(conn: &Connection, id: &str) -> Result<Option<Prompt>, DbError> {
    Ok(prompts_of(conn, sql::PROMPT_GET, params![id])?
        .into_iter()
        .next())
}

/// Version `version` of prompt `id` with its findings, if there is one.
fn version_of(conn: &Connection, id: &str, version: u32) -> Result<Option<VersionRecord>, DbError> {
    let found: Vec<VersionRecord> = read_rows(
        conn,
        sql::PROMPT_VERSION_GET,
        version_def(),
        params![id, i64::from(version)],
    )?;
    let Some(mut record) = found.into_iter().next() else {
        return Ok(None);
    };
    record.findings = read_rows(
        conn,
        sql::PROMPT_FINDINGS,
        findings_def(),
        params![id, i64::from(version)],
    )?;
    Ok(Some(record))
}

/// The body of version `version` of prompt `id`, if there is such a version.
fn body_of(conn: &Connection, id: &str, version: u32) -> Result<Option<String>, DbError> {
    Ok(conn
        .prepare_cached(sql::PROMPT_BODY)?
        .query_row(params![id, i64::from(version)], |r| r.get(0))
        .optional()?)
}

/// The id of another live prompt holding `binding`, if any.
fn binding_holder(conn: &Connection, binding: &str, id: &str) -> Result<Option<String>, DbError> {
    Ok(conn
        .prepare_cached(sql::PROMPT_BINDING_HOLDER)?
        .query_row(params![binding, id], |r| r.get(0))
        .optional()?)
}

/// Writes a record and its child rows as upserts: its root row at `pos` (the table's place for the row), then every
/// child table's rows. A list is written whole by its caller (`save_prompt` clears the tags first).
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

/// Writes a prompt's row and its tags. The row keeps its place in the list (its first stored place; a new prompt takes
/// the next one); the tags are written whole.
fn save_prompt(conn: &Connection, prompt: &Prompt) -> Result<(), DbError> {
    let place: i64 = conn
        .prepare_cached(sql::PROMPT_POS)?
        .query_row(params![prompt.id], |r| r.get(0))?;
    conn.prepare_cached(sql::PROMPT_TAGS_CLEAR)?
        .execute(params![prompt.id])?;
    save(conn, def(), prompt, u64::try_from(place).unwrap_or(0))
}

/// Writes a new version (immutable: a version row is never rewritten) with its findings.
fn save_version(conn: &Connection, record: &VersionRecord) -> Result<(), DbError> {
    save(conn, version_def(), record, u64::from(record.version))
}

/// Appends one history row. Its id is the next after the newest row of any prompt, so the history is one stream.
fn record_event(
    conn: &Connection,
    prompt_id: &str,
    action: Action,
    version: Option<u32>,
    actor: &str,
    at_ms: u64,
) -> Result<(), DbError> {
    let last: i64 = conn
        .prepare_cached(sql::PROMPT_EVENT_LAST_SEQ)?
        .query_row([], |r| r.get(0))?;
    let id = u64::try_from(last).unwrap_or(0) + 1;
    let event = Event {
        id,
        prompt_id: prompt_id.to_string(),
        action,
        version,
        actor: actor.to_string(),
        at_ms,
    };
    save(conn, event_def(), &event, id)
}
