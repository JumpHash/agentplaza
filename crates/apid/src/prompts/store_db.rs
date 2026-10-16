//! `PromptStore` in Postgres (migration 0006, `sql/queries/prompt_*.sql`). A write runs in one transaction with its
//! history row: the prompt row is locked with `FOR UPDATE`, every check is a query inside that transaction, and the
//! closure returns an `Outcome`. Only `Done` commits, so a refused write leaves nothing behind.

use std::error::Error;
use std::sync::Arc;

use apz_prompt::{Finding, Kind, Severity};
use postgres::types::{FromSql, Type};
use postgres::{Client, Row, Transaction};

use super::{
    Action, Event, Meta, NewPrompt, NewVersion, Prompt, PromptStore, StoreError, Version,
    VersionMeta,
};
use crate::db::Db;
use crate::db::sql;

pub struct DbPrompts {
    db: Arc<Db>,
}

impl DbPrompts {
    pub fn new(db: Arc<Db>) -> DbPrompts {
        DbPrompts { db }
    }

    /// Runs one write. Only `Done` commits; the other outcomes become the store's errors.
    fn write<T>(
        &self,
        op: impl FnMut(&mut Client) -> Result<Outcome<T>, postgres::Error>,
    ) -> Result<T, StoreError> {
        match self.db.run(op)? {
            Outcome::Done(value) => Ok(value),
            Outcome::NotFound => Err(StoreError::NotFound),
            Outcome::Conflict(msg) => Err(StoreError::Conflict(msg)),
            Outcome::Invalid(msg) => Err(StoreError::Invalid(msg)),
        }
    }
}

/// What a write decided inside its transaction.
enum Outcome<T> {
    Done(T),
    NotFound,
    Conflict(String),
    Invalid(String),
}

/// A text column read through `Named::named`; an unknown value fails the read like any other bad column.
struct Parsed<T>(T);

trait Named: Sized {
    fn named(text: &str) -> Option<Self>;
}

impl Named for Kind {
    fn named(text: &str) -> Option<Self> {
        Kind::parse(text)
    }
}

impl Named for Severity {
    fn named(text: &str) -> Option<Self> {
        Severity::parse(text)
    }
}

impl Named for Action {
    fn named(text: &str) -> Option<Self> {
        Action::parse(text)
    }
}

impl<'a, T: Named> FromSql<'a> for Parsed<T> {
    fn from_sql(_: &Type, raw: &'a [u8]) -> Result<Self, Box<dyn Error + Sync + Send>> {
        let text = std::str::from_utf8(raw)?;
        T::named(text)
            .map(Parsed)
            .ok_or_else(|| format!("unknown value {text:?}").into())
    }

    fn accepts(ty: &Type) -> bool {
        *ty == Type::TEXT
    }
}

impl PromptStore for DbPrompts {
    fn list(&self, archived: bool) -> Result<Vec<Prompt>, String> {
        self.db.run(|c| {
            let rows = c.query(sql::PROMPT_LIST, &[&archived])?;
            rows.iter().map(prompt_of).collect::<Result<Vec<_>, _>>()
        })
    }

    fn get(&self, id: &str) -> Result<Option<Prompt>, String> {
        self.db.run(|c| {
            c.query_opt(sql::PROMPT_GET, &[&id])?
                .map(|row| prompt_of(&row))
                .transpose()
        })
    }

    fn versions(&self, id: &str) -> Result<Vec<VersionMeta>, String> {
        self.db.run(|c| {
            let rows = c.query(sql::PROMPT_VERSIONS, &[&id])?;
            rows.iter().map(meta_of).collect::<Result<Vec<_>, _>>()
        })
    }

    fn version(&self, id: &str, version: u32) -> Result<Option<Version>, String> {
        self.db.run(|c| {
            let Some(row) = c.query_opt(sql::PROMPT_VERSION_GET, &[&id, &(version as i32)])? else {
                return Ok(None);
            };
            let meta = meta_of(&row)?;
            let body: String = row.try_get(10)?;
            let rows = c.query(sql::PROMPT_FINDINGS, &[&id, &(version as i32)])?;
            let findings = rows.iter().map(finding_of).collect::<Result<Vec<_>, _>>()?;
            Ok(Some(Version {
                meta,
                body,
                findings,
            }))
        })
    }

    fn create(
        &self,
        new: &NewPrompt,
        first: &NewVersion,
        actor: &str,
        now_ms: u64,
    ) -> Result<Prompt, StoreError> {
        self.write(|c| {
            let mut tx = c.transaction()?;
            if lock_prompt(&mut tx, &new.id)?.is_some() {
                return Ok(Outcome::Conflict(format!(
                    "a prompt named {} already exists",
                    new.id
                )));
            }
            if let Some(binding) = new.binding.as_deref()
                && let Some(holder) = binding_holder(&mut tx, binding, &new.id)?
            {
                return Ok(Outcome::Conflict(format!(
                    "binding {binding} is held by {holder}"
                )));
            }
            let now = now_ms as i64;
            tx.execute(
                sql::PROMPT_INSERT,
                &[
                    &new.id,
                    &new.kind.as_str(),
                    &new.title,
                    &new.description,
                    &new.tags,
                    &new.binding,
                    &now,
                ],
            )?;
            insert_version(&mut tx, &new.id, 1, first, actor, now)?;
            insert_event(&mut tx, &new.id, Action::Create, None, actor, now)?;
            insert_event(&mut tx, &new.id, Action::Version, Some(1), actor, now)?;
            let prompt = reread(&mut tx, &new.id)?;
            tx.commit()?;
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
        self.write(|c| {
            let mut tx = c.transaction()?;
            let Some(prompt) = lock_prompt(&mut tx, id)? else {
                return Ok(Outcome::NotFound);
            };
            if prompt.latest_version != base {
                return Ok(Outcome::Conflict(format!(
                    "{id} is at version {}, not {base}",
                    prompt.latest_version
                )));
            }
            let version = base + 1;
            let now = now_ms as i64;
            insert_version(&mut tx, id, version as i32, next, actor, now)?;
            tx.execute(sql::PROMPT_SET_LATEST, &[&id, &(version as i32), &now])?;
            insert_event(
                &mut tx,
                id,
                Action::Version,
                Some(version as i32),
                actor,
                now,
            )?;
            tx.commit()?;
            let report = &next.report;
            Ok(Outcome::Done(VersionMeta {
                version,
                note: next.note.clone(),
                author: actor.to_string(),
                created_ms: now_ms,
                score: report.score,
                errors: report.errors,
                warnings: report.warnings,
                infos: report.infos,
                rules_version: report.rules_version,
                sha256: next.sha256.clone(),
            }))
        })
    }

    fn set_meta(
        &self,
        id: &str,
        meta: &Meta,
        actor: &str,
        now_ms: u64,
    ) -> Result<Prompt, StoreError> {
        self.write(|c| {
            let mut tx = c.transaction()?;
            if lock_prompt(&mut tx, id)?.is_none() {
                return Ok(Outcome::NotFound);
            }
            if let Some(binding) = meta.binding.as_deref()
                && let Some(holder) = binding_holder(&mut tx, binding, id)?
            {
                return Ok(Outcome::Conflict(format!(
                    "binding {binding} is held by {holder}"
                )));
            }
            let now = now_ms as i64;
            tx.execute(
                sql::PROMPT_SET_META,
                &[
                    &id,
                    &meta.title,
                    &meta.description,
                    &meta.tags,
                    &meta.binding,
                    &now,
                ],
            )?;
            insert_event(&mut tx, id, Action::Edit, None, actor, now)?;
            let prompt = reread(&mut tx, id)?;
            tx.commit()?;
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
        self.write(|c| {
            let mut tx = c.transaction()?;
            if lock_prompt(&mut tx, id)?.is_none() {
                return Ok(Outcome::NotFound);
            }
            let version = version.map(|v| v as i32);
            let now = now_ms as i64;
            let action = match version {
                Some(v) => {
                    if tx.query_opt(sql::PROMPT_VERSION_GET, &[&id, &v])?.is_none() {
                        return Ok(Outcome::Invalid(format!("{id} has no version {v}")));
                    }
                    Action::Publish
                }
                None => Action::Unpublish,
            };
            tx.execute(sql::PROMPT_SET_ACTIVE, &[&id, &version, &now])?;
            insert_event(&mut tx, id, action, version, actor, now)?;
            let prompt = reread(&mut tx, id)?;
            tx.commit()?;
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
        self.write(|c| {
            let mut tx = c.transaction()?;
            let Some(prompt) = lock_prompt(&mut tx, id)? else {
                return Ok(Outcome::NotFound);
            };
            if !archived
                && let Some(binding) = prompt.binding.as_deref()
                && let Some(holder) = binding_holder(&mut tx, binding, id)?
            {
                return Ok(Outcome::Conflict(format!(
                    "binding {binding} is held by {holder}"
                )));
            }
            let now = now_ms as i64;
            tx.execute(sql::PROMPT_SET_ARCHIVED, &[&id, &archived, &now])?;
            let action = if archived {
                Action::Archive
            } else {
                Action::Restore
            };
            insert_event(&mut tx, id, action, None, actor, now)?;
            let prompt = reread(&mut tx, id)?;
            tx.commit()?;
            Ok(Outcome::Done(prompt))
        })
    }

    fn events(&self, id: &str, limit: u32) -> Result<Vec<Event>, String> {
        self.db.run(|c| {
            let rows = c.query(sql::PROMPT_EVENTS, &[&id, &i64::from(limit)])?;
            rows.iter().map(event_of).collect::<Result<Vec<_>, _>>()
        })
    }

    fn bound(&self, binding: &str) -> Result<Option<(Prompt, String)>, String> {
        self.db.run(|c| {
            let Some(row) = c.query_opt(sql::PROMPT_BOUND, &[&binding])? else {
                return Ok(None);
            };
            let prompt = prompt_of(&row)?;
            let body: String = row.try_get(11)?;
            Ok(Some((prompt, body)))
        })
    }
}

/// Locks the prompt row until the transaction ends; `None` when there is no such prompt.
fn lock_prompt(tx: &mut Transaction<'_>, id: &str) -> Result<Option<Prompt>, postgres::Error> {
    tx.query_opt(sql::PROMPT_LOCK, &[&id])?
        .map(|row| prompt_of(&row))
        .transpose()
}

/// The prompt row as `prompt_get.sql` reads it, inside the transaction.
fn reread(tx: &mut Transaction<'_>, id: &str) -> Result<Prompt, postgres::Error> {
    let row = tx.query_one(sql::PROMPT_GET, &[&id])?;
    prompt_of(&row)
}

/// The id of another live prompt holding `binding`, if any.
fn binding_holder(
    tx: &mut Transaction<'_>,
    binding: &str,
    id: &str,
) -> Result<Option<String>, postgres::Error> {
    tx.query_opt(sql::PROMPT_BINDING_HOLDER, &[&binding, &id])?
        .map(|row| row.try_get::<_, String>(0))
        .transpose()
}

/// Writes a version, its check score and its findings (seq from 0).
fn insert_version(
    tx: &mut Transaction<'_>,
    id: &str,
    version: i32,
    next: &NewVersion,
    actor: &str,
    now: i64,
) -> Result<(), postgres::Error> {
    let report = &next.report;
    tx.execute(
        sql::PROMPT_VERSION_INSERT,
        &[
            &id,
            &version,
            &next.body,
            &next.sha256,
            &next.note,
            &actor,
            &(report.score as i16),
            &(report.errors as i32),
            &(report.warnings as i32),
            &(report.infos as i32),
            &(report.rules_version as i32),
            &now,
        ],
    )?;
    for (seq, finding) in report.findings.iter().enumerate() {
        tx.execute(
            sql::PROMPT_FINDING_INSERT,
            &[
                &id,
                &version,
                &(seq as i32),
                &finding.rule,
                &finding.severity.as_str(),
                &(finding.line as i32),
                &finding.message,
            ],
        )?;
    }
    Ok(())
}

fn insert_event(
    tx: &mut Transaction<'_>,
    id: &str,
    action: Action,
    version: Option<i32>,
    actor: &str,
    now: i64,
) -> Result<(), postgres::Error> {
    tx.execute(
        sql::PROMPT_EVENT_INSERT,
        &[&id, &action.as_str(), &version, &actor, &now],
    )?;
    Ok(())
}

fn prompt_of(row: &Row) -> Result<Prompt, postgres::Error> {
    let kind: Parsed<Kind> = row.try_get(1)?;
    let active: Option<i32> = row.try_get(6)?;
    let latest: i32 = row.try_get(7)?;
    let archived: Option<i64> = row.try_get(10)?;
    Ok(Prompt {
        id: row.try_get(0)?,
        kind: kind.0,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        tags: row.try_get(4)?,
        binding: row.try_get(5)?,
        active_version: active.map(|v| v as u32),
        latest_version: latest as u32,
        created_ms: row.try_get::<_, i64>(8)? as u64,
        updated_ms: row.try_get::<_, i64>(9)? as u64,
        archived_ms: archived.map(|v| v as u64),
    })
}

/// The metadata columns of `prompt_versions.sql` (the first ten of `prompt_version_get.sql`).
fn meta_of(row: &Row) -> Result<VersionMeta, postgres::Error> {
    Ok(VersionMeta {
        version: row.try_get::<_, i32>(0)? as u32,
        note: row.try_get(1)?,
        author: row.try_get(2)?,
        created_ms: row.try_get::<_, i64>(3)? as u64,
        score: row.try_get::<_, i16>(4)? as u8,
        errors: row.try_get::<_, i32>(5)? as u32,
        warnings: row.try_get::<_, i32>(6)? as u32,
        infos: row.try_get::<_, i32>(7)? as u32,
        rules_version: row.try_get::<_, i32>(8)? as u32,
        sha256: row.try_get(9)?,
    })
}

fn finding_of(row: &Row) -> Result<Finding, postgres::Error> {
    let rule: String = row.try_get(0)?;
    let severity: Parsed<Severity> = row.try_get(1)?;
    let line: i32 = row.try_get(2)?;
    let message: String = row.try_get(3)?;
    Ok(Finding::new(&rule, severity.0, line as u32, message))
}

fn event_of(row: &Row) -> Result<Event, postgres::Error> {
    let action: Parsed<Action> = row.try_get(2)?;
    let version: Option<i32> = row.try_get(3)?;
    Ok(Event {
        id: row.try_get::<_, i64>(0)? as u64,
        prompt_id: row.try_get(1)?,
        action: action.0,
        version: version.map(|v| v as u32),
        actor: row.try_get(4)?,
        at_ms: row.try_get::<_, i64>(5)? as u64,
    })
}
