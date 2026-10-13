//! `SkillStore` in Postgres (migration 0007, `sql/queries/skill_*.sql`). A write runs in one transaction with its
//! history rows: the skill row is locked with `FOR UPDATE`, every check is a query inside that transaction, and the
//! closure returns an `Outcome`. Only `Done` commits, so a refused write leaves nothing behind.

use std::error::Error;
use std::sync::Arc;

use apz_skill::{Finding, Severity};
use postgres::types::{FromSql, Type};
use postgres::{Client, Row, Transaction};

use super::{Action, Event, NewSkill, NewVersion, Skill, SkillFile, SkillStore, Source, StoreError, Version, VersionMeta};
use crate::db::sql;
use crate::db::Db;

pub struct DbSkills {
    db: Arc<Db>,
}

impl DbSkills {
    pub fn new(db: Arc<Db>) -> DbSkills {
        DbSkills { db }
    }

    /// Runs one write. Only `Done` commits; the other outcomes become the store's errors.
    fn write<T>(&self, op: impl FnMut(&mut Client) -> Result<Outcome<T>, postgres::Error>) -> Result<T, StoreError> {
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

impl Named for Action {
    fn named(text: &str) -> Option<Self> {
        Action::parse(text)
    }
}

impl Named for Severity {
    fn named(text: &str) -> Option<Self> {
        Severity::parse(text)
    }
}

/// The `source_kind` column.
#[derive(Clone, Copy)]
enum SourceKind {
    Built,
    Upload,
    Github,
}

impl Named for SourceKind {
    fn named(text: &str) -> Option<Self> {
        match text {
            "built" => Some(SourceKind::Built),
            "upload" => Some(SourceKind::Upload),
            "github" => Some(SourceKind::Github),
            _ => None,
        }
    }
}

impl<'a, T: Named> FromSql<'a> for Parsed<T> {
    fn from_sql(_: &Type, raw: &'a [u8]) -> Result<Self, Box<dyn Error + Sync + Send>> {
        let text = std::str::from_utf8(raw)?;
        T::named(text).map(Parsed).ok_or_else(|| format!("unknown value {text:?}").into())
    }

    fn accepts(ty: &Type) -> bool {
        *ty == Type::TEXT
    }
}

impl SkillStore for DbSkills {
    fn list(&self, archived: bool) -> Result<Vec<Skill>, String> {
        self.db.run(|c| {
            let rows = c.query(sql::SKILL_LIST, &[&archived])?;
            rows.iter().map(skill_of).collect::<Result<Vec<_>, _>>()
        })
    }

    fn get(&self, id: &str) -> Result<Option<Skill>, String> {
        self.db.run(|c| c.query_opt(sql::SKILL_GET, &[&id])?.map(|row| skill_of(&row)).transpose())
    }

    fn versions(&self, id: &str) -> Result<Vec<VersionMeta>, String> {
        self.db.run(|c| {
            let rows = c.query(sql::SKILL_VERSIONS, &[&id])?;
            rows.iter().map(|row| meta_of(row, 0)).collect::<Result<Vec<_>, _>>()
        })
    }

    fn version(&self, id: &str, version: u32) -> Result<Option<Version>, String> {
        self.db.run(|c| {
            let Some(row) = c.query_opt(sql::SKILL_VERSION_GET, &[&id, &(version as i32)])? else {
                return Ok(None);
            };
            let meta = meta_of(&row, 0)?;
            let skill_md: String = row.try_get(13)?;
            let file_rows = c.query(sql::SKILL_FILES, &[&id, &(version as i32)])?;
            let files = file_rows.iter().map(file_of).collect::<Result<Vec<_>, _>>()?;
            let finding_rows = c.query(sql::SKILL_FINDINGS, &[&id, &(version as i32)])?;
            let findings = finding_rows.iter().map(finding_of).collect::<Result<Vec<_>, _>>()?;
            Ok(Some(Version { meta, skill_md, files, findings }))
        })
    }

    fn create(&self, new: &NewSkill, first: &NewVersion, actor: &str, now_ms: u64) -> Result<Skill, StoreError> {
        self.write(|c| {
            let mut tx = c.transaction()?;
            if lock_skill(&mut tx, &new.id)?.is_some() {
                return Ok(Outcome::Conflict(format!("a skill with id {} already exists", new.id)));
            }
            let name = name_of(first, &new.id);
            if let Some(holder) = name_holder(&mut tx, &name, &new.id)? {
                return Ok(Outcome::Conflict(format!("name {name} is held by {holder}")));
            }
            let now = now_ms as i64;
            let description = first.report.description.clone().unwrap_or_default();
            let (kind, repo, git_ref, path, commit) = source_columns(&new.source);
            tx.execute(
                sql::SKILL_INSERT,
                &[&new.id, &name, &new.title, &description, &new.tags, &kind, &repo, &git_ref, &path, &commit, &now],
            )?;
            insert_version(&mut tx, &new.id, 1, first, actor, now)?;
            insert_event(&mut tx, &new.id, Action::Create, None, actor, now)?;
            insert_event(&mut tx, &new.id, Action::Version, Some(1), actor, now)?;
            for binding in &new.bindings {
                tx.execute(sql::SKILL_BINDINGS_INSERT, &[&new.id, binding])?;
            }
            let skill = reread(&mut tx, &new.id)?;
            tx.commit()?;
            Ok(Outcome::Done(skill))
        })
    }

    fn add_version(&self, id: &str, base: u32, next: &NewVersion, actor: &str, now_ms: u64) -> Result<VersionMeta, StoreError> {
        self.write(|c| {
            let mut tx = c.transaction()?;
            let Some(skill) = lock_skill(&mut tx, id)? else {
                return Ok(Outcome::NotFound);
            };
            if skill.latest_version != base {
                return Ok(Outcome::Conflict(format!("{id} is at version {}, not {base}", skill.latest_version)));
            }
            let name = name_of(next, id);
            if let Some(holder) = name_holder(&mut tx, &name, id)? {
                return Ok(Outcome::Conflict(format!("name {name} is held by {holder}")));
            }
            let version = base + 1;
            let now = now_ms as i64;
            let description = next.report.description.clone().unwrap_or_default();
            insert_version(&mut tx, id, version as i32, next, actor, now)?;
            tx.execute(
                sql::SKILL_SET_LATEST,
                &[&id, &(version as i32), &name, &description, &next.source_commit, &now],
            )?;
            insert_event(&mut tx, id, Action::Version, Some(version as i32), actor, now)?;
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
                file_count: next.files.len() as u32,
                total_bytes: total_bytes(next),
                source_commit: next.source_commit.clone(),
            }))
        })
    }

    fn set_meta(&self, id: &str, title: &str, tags: &[String], actor: &str, now_ms: u64) -> Result<Skill, StoreError> {
        self.write(|c| {
            let mut tx = c.transaction()?;
            if lock_skill(&mut tx, id)?.is_none() {
                return Ok(Outcome::NotFound);
            }
            let now = now_ms as i64;
            tx.execute(sql::SKILL_SET_META, &[&id, &title, &tags, &now])?;
            insert_event(&mut tx, id, Action::Edit, None, actor, now)?;
            let skill = reread(&mut tx, id)?;
            tx.commit()?;
            Ok(Outcome::Done(skill))
        })
    }

    fn set_bindings(&self, id: &str, bindings: &[String], actor: &str, now_ms: u64) -> Result<Skill, StoreError> {
        self.write(|c| {
            let mut tx = c.transaction()?;
            if lock_skill(&mut tx, id)?.is_none() {
                return Ok(Outcome::NotFound);
            }
            let now = now_ms as i64;
            tx.execute(sql::SKILL_BINDINGS_DELETE, &[&id])?;
            for binding in bindings {
                tx.execute(sql::SKILL_BINDINGS_INSERT, &[&id, binding])?;
            }
            tx.execute(sql::SKILL_TOUCH, &[&id, &now])?;
            insert_event(&mut tx, id, Action::Bind, None, actor, now)?;
            let skill = reread(&mut tx, id)?;
            tx.commit()?;
            Ok(Outcome::Done(skill))
        })
    }

    fn set_active(&self, id: &str, version: Option<u32>, actor: &str, now_ms: u64) -> Result<Skill, StoreError> {
        self.write(|c| {
            let mut tx = c.transaction()?;
            if lock_skill(&mut tx, id)?.is_none() {
                return Ok(Outcome::NotFound);
            }
            let version = version.map(|v| v as i32);
            let now = now_ms as i64;
            let action = match version {
                Some(v) => {
                    if tx.query_opt(sql::SKILL_VERSION_GET, &[&id, &v])?.is_none() {
                        return Ok(Outcome::Invalid(format!("{id} has no version {v}")));
                    }
                    Action::Publish
                }
                None => Action::Unpublish,
            };
            tx.execute(sql::SKILL_SET_ACTIVE, &[&id, &version, &now])?;
            insert_event(&mut tx, id, action, version, actor, now)?;
            let skill = reread(&mut tx, id)?;
            tx.commit()?;
            Ok(Outcome::Done(skill))
        })
    }

    fn set_archived(&self, id: &str, archived: bool, actor: &str, now_ms: u64) -> Result<Skill, StoreError> {
        self.write(|c| {
            let mut tx = c.transaction()?;
            let Some(skill) = lock_skill(&mut tx, id)? else {
                return Ok(Outcome::NotFound);
            };
            if !archived {
                if let Some(holder) = name_holder(&mut tx, &skill.name, id)? {
                    return Ok(Outcome::Conflict(format!("name {} is held by {holder}", skill.name)));
                }
            }
            let now = now_ms as i64;
            tx.execute(sql::SKILL_SET_ARCHIVED, &[&id, &archived, &now])?;
            let action = if archived { Action::Archive } else { Action::Restore };
            insert_event(&mut tx, id, action, None, actor, now)?;
            let skill = reread(&mut tx, id)?;
            tx.commit()?;
            Ok(Outcome::Done(skill))
        })
    }

    fn events(&self, id: &str, limit: u32) -> Result<Vec<Event>, String> {
        self.db.run(|c| {
            let rows = c.query(sql::SKILL_EVENTS, &[&id, &i64::from(limit)])?;
            rows.iter().map(event_of).collect::<Result<Vec<_>, _>>()
        })
    }

    fn bound(&self, binding: &str) -> Result<Vec<(Skill, VersionMeta)>, String> {
        self.db.run(|c| {
            let rows = c.query(sql::SKILL_BOUND, &[&binding])?;
            rows.iter().map(|row| Ok((skill_of(row)?, meta_of(row, 16)?))).collect::<Result<Vec<_>, postgres::Error>>()
        })
    }
}

/// Locks the skill row until the transaction ends; `None` when there is no such skill.
fn lock_skill(tx: &mut Transaction<'_>, id: &str) -> Result<Option<Skill>, postgres::Error> {
    tx.query_opt(sql::SKILL_LOCK, &[&id])?.map(|row| skill_of(&row)).transpose()
}

/// The skill row as `skill_get.sql` reads it, inside the transaction.
fn reread(tx: &mut Transaction<'_>, id: &str) -> Result<Skill, postgres::Error> {
    let row = tx.query_one(sql::SKILL_GET, &[&id])?;
    skill_of(&row)
}

/// The id of another live skill holding `name`, if any.
fn name_holder(tx: &mut Transaction<'_>, name: &str, id: &str) -> Result<Option<String>, postgres::Error> {
    tx.query_opt(sql::SKILL_NAME_HOLDER, &[&name, &id])?.map(|row| row.try_get::<_, String>(0)).transpose()
}

/// A version's name is its frontmatter name, or the skill id when it has none.
fn name_of(next: &NewVersion, id: &str) -> String {
    next.report.name.clone().unwrap_or_else(|| id.to_string())
}

fn total_bytes(next: &NewVersion) -> u64 {
    next.files.iter().map(|f| f.size).sum()
}

/// The source columns: kind, repo (`owner/repo`), ref, path and commit (the last four null unless GitHub).
fn source_columns(source: &Source) -> (&str, Option<&str>, Option<&str>, Option<&str>, Option<&str>) {
    match source {
        Source::Built => ("built", None, None, None, None),
        Source::Upload => ("upload", None, None, None, None),
        Source::Github { repo, git_ref, path, commit } => {
            ("github", Some(repo.as_str()), git_ref.as_deref(), Some(path.as_str()), Some(commit.as_str()))
        }
    }
}

/// Writes a version, its files and its check findings (seq from 0).
fn insert_version(tx: &mut Transaction<'_>, id: &str, version: i32, next: &NewVersion, actor: &str, now: i64) -> Result<(), postgres::Error> {
    let report = &next.report;
    tx.execute(
        sql::SKILL_VERSION_INSERT,
        &[
            &id,
            &version,
            &next.skill_md,
            &next.sha256,
            &next.note,
            &actor,
            &(report.score as i16),
            &(report.errors as i32),
            &(report.warnings as i32),
            &(report.infos as i32),
            &(report.rules_version as i32),
            &(next.files.len() as i32),
            &(total_bytes(next) as i64),
            &next.source_commit,
            &now,
        ],
    )?;
    for f in &next.files {
        tx.execute(sql::SKILL_FILE_INSERT, &[&id, &version, &f.path, &f.sha256, &(f.size as i64), &f.executable])?;
    }
    for (seq, finding) in report.findings.iter().enumerate() {
        tx.execute(
            sql::SKILL_FINDING_INSERT,
            &[
                &id,
                &version,
                &(seq as i32),
                &finding.rule,
                &finding.severity.as_str(),
                &finding.path,
                &(finding.line as i32),
                &finding.message,
            ],
        )?;
    }
    Ok(())
}

fn insert_event(tx: &mut Transaction<'_>, id: &str, action: Action, version: Option<i32>, actor: &str, now: i64) -> Result<(), postgres::Error> {
    tx.execute(sql::SKILL_EVENT_INSERT, &[&id, &action.as_str(), &version, &actor, &now])?;
    Ok(())
}

/// The skill columns of `skill_get.sql` (0 to 15).
fn skill_of(row: &Row) -> Result<Skill, postgres::Error> {
    let active: Option<i32> = row.try_get(5)?;
    let latest: i32 = row.try_get(6)?;
    let archived: Option<i64> = row.try_get(14)?;
    Ok(Skill {
        id: row.try_get(0)?,
        name: row.try_get(1)?,
        title: row.try_get(2)?,
        description: row.try_get(3)?,
        tags: row.try_get(4)?,
        bindings: row.try_get(15)?,
        active_version: active.map(|v| v as u32),
        latest_version: latest as u32,
        source: source_of(row)?,
        created_ms: row.try_get::<_, i64>(12)? as u64,
        updated_ms: row.try_get::<_, i64>(13)? as u64,
        archived_ms: archived.map(|v| v as u64),
    })
}

/// The source columns (7 to 11) of a skill row.
fn source_of(row: &Row) -> Result<Source, postgres::Error> {
    let kind: Parsed<SourceKind> = row.try_get(7)?;
    Ok(match kind.0 {
        SourceKind::Built => Source::Built,
        SourceKind::Upload => Source::Upload,
        SourceKind::Github => Source::Github {
            repo: row.try_get::<_, Option<String>>(8)?.unwrap_or_default(),
            git_ref: row.try_get(9)?,
            path: row.try_get::<_, Option<String>>(10)?.unwrap_or_default(),
            commit: row.try_get::<_, Option<String>>(11)?.unwrap_or_default(),
        },
    })
}

/// The metadata columns of a version (from column `at`: `skill_versions.sql` reads them from 0, `skill_bound.sql`
/// from 16).
fn meta_of(row: &Row, at: usize) -> Result<VersionMeta, postgres::Error> {
    Ok(VersionMeta {
        version: row.try_get::<_, i32>(at)? as u32,
        note: row.try_get(at + 1)?,
        author: row.try_get(at + 2)?,
        created_ms: row.try_get::<_, i64>(at + 3)? as u64,
        score: row.try_get::<_, i16>(at + 4)? as u8,
        errors: row.try_get::<_, i32>(at + 5)? as u32,
        warnings: row.try_get::<_, i32>(at + 6)? as u32,
        infos: row.try_get::<_, i32>(at + 7)? as u32,
        rules_version: row.try_get::<_, i32>(at + 8)? as u32,
        sha256: row.try_get(at + 9)?,
        file_count: row.try_get::<_, i32>(at + 10)? as u32,
        total_bytes: row.try_get::<_, i64>(at + 11)? as u64,
        source_commit: row.try_get(at + 12)?,
    })
}

fn file_of(row: &Row) -> Result<SkillFile, postgres::Error> {
    Ok(SkillFile {
        path: row.try_get(0)?,
        sha256: row.try_get(1)?,
        size: row.try_get::<_, i64>(2)? as u64,
        executable: row.try_get(3)?,
    })
}

fn finding_of(row: &Row) -> Result<Finding, postgres::Error> {
    let severity: Parsed<Severity> = row.try_get(1)?;
    Ok(Finding {
        rule: row.try_get(0)?,
        severity: severity.0,
        path: row.try_get(2)?,
        line: row.try_get::<_, i32>(3)? as u32,
        message: row.try_get(4)?,
    })
}

fn event_of(row: &Row) -> Result<Event, postgres::Error> {
    let action: Parsed<Action> = row.try_get(2)?;
    let version: Option<i32> = row.try_get(3)?;
    Ok(Event {
        id: row.try_get::<_, i64>(0)? as u64,
        skill_id: row.try_get(1)?,
        action: action.0,
        version: version.map(|v| v as u32),
        actor: row.try_get(4)?,
        at_ms: row.try_get::<_, i64>(5)? as u64,
    })
}
