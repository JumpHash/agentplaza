//! The owner's standing rules (PLAN 9.8, Engine 2.6): one text the owner edits in the IDE, kept with its versions.
//! Part 2 of every session's system prompt carries the text in force (agents.rs `system_parts`), and so does every
//! engine's instruction file, since the engine adapters render the parts (1.9). The text replaces the house rules,
//! the prompt library's AGENTS.md bound to the plaza: at start, when no version exists yet, that text becomes the
//! first one (`absorb`).
//!
//! GET /v1/rules/standing → {text, version, author, updated_ms, versions: [{version, author, at_ms}]}
//! PUT /v1/rules/standing {text} → saves the text as a new version; the text in force again saves nothing
//!
//! Kept in the hub's store (component `standing_rules`): the versions are child rows of one root row. The text is a
//! plain column, not `content`: it is read back at start.

use std::sync::{Mutex, MutexGuard, PoisonError};

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_store::{RowSet, StoreError, TableDef};
use serde_json::{Value, json};

use crate::state::App;

/// The longest text the owner may keep (characters): it goes into every session's prompt.
pub const MAX_TEXT: usize = 20_000;

/// One saved version of the standing rules.
#[derive(Clone, Debug, Default, PartialEq, Eq, apz_store::Table)]
pub struct Version {
    /// Its number (1, 2, …).
    pub seq: u64,
    pub text: String,
    /// Who saved it: `owner`, or `house rules` for the text absorbed at start.
    pub author: String,
    pub at_ms: u64,
}

/// The standing rules' tables: `standing_rules`, with each version as a child row.
#[derive(Clone, Debug, Default, PartialEq, Eq, apz_store::Table)]
#[table(name = "standing_rules")]
pub struct Record {
    /// Oldest first; the last one is the text in force.
    #[table(child = "standing_rule")]
    pub versions: Vec<Version>,
}

/// The owner's standing rules, as the hub holds them.
#[derive(Default)]
pub struct StandingRules {
    inner: Mutex<Record>,
}

impl StandingRules {
    fn lock(&self) -> MutexGuard<'_, Record> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replaces the record with what plaza.db holds (at start).
    pub fn load(&self, conn: &apz_store::rusqlite::Connection) -> Result<(), StoreError> {
        let stored: Record = apz_store::table::load_one(conn, def())?.unwrap_or_default();
        *self.lock() = stored;
        Ok(())
    }

    /// The record as rows of its tables (the saver writes what changed).
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        RowSet::one(def(), &*self.lock())
    }

    /// The text in force: the newest version's, or nothing when none was saved.
    pub fn text(&self) -> String {
        self.lock()
            .versions
            .last()
            .map(|v| v.text.clone())
            .unwrap_or_default()
    }

    /// The whole record: every version, oldest first.
    pub fn snapshot(&self) -> Record {
        self.lock().clone()
    }

    /// Saves `text` as a new version, unless it is the text in force already (then nothing is saved). Returns the new
    /// version's number.
    pub fn save(&self, text: &str, author: &str, now_ms: u64) -> Result<Option<u64>, String> {
        let text = text.trim();
        if text.chars().count() > MAX_TEXT {
            return Err(format!("at most {MAX_TEXT} characters"));
        }
        let mut record = self.lock();
        if record.versions.last().is_some_and(|v| v.text == text) {
            return Ok(None);
        }
        Ok(Some(push(&mut record, text, author, now_ms)))
    }

    /// Adopts the house rules' text as the first version, when no version exists yet (state.rs calls it at start).
    /// The house rules were in every prompt already, so the text is kept whole, the limit not applied. Returns whether
    /// it did.
    pub fn absorb(&self, house_rules: &str, now_ms: u64) -> bool {
        let text = house_rules.trim();
        let mut record = self.lock();
        if !record.versions.is_empty() || text.is_empty() {
            return false;
        }
        push(&mut record, text, "house rules", now_ms);
        true
    }
}

/// Appends a version holding `text` to `record`; returns its number.
fn push(record: &mut Record, text: &str, author: &str, now_ms: u64) -> u64 {
    let seq = record.versions.len() as u64 + 1;
    record.versions.push(Version {
        seq,
        text: text.to_string(),
        author: author.to_string(),
        at_ms: now_ms,
    });
    seq
}

/// The standing rules' tables.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(Record)
}

/// The standing rules' routes. Owner-only, like the rest of `/v1/` (`hub_auth::gate`).
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/rules/standing", standing_route)
        .doc(RouteDoc::new(
            "The owner's standing rules: the text in force and its versions",
        ))
        .route("PUT", "/v1/rules/standing", standing_save_route)
        .doc(RouteDoc::new("Save the standing rules as a new version").errors(&[400]))
}

/// `GET /v1/rules/standing`: the text in force and its history.
fn standing_route(app: &App, _: &Request, _: &Params<'_>) -> Response {
    Response::json(200, &view(app, None))
}

/// `PUT /v1/rules/standing` with `{text}`: saves the text as a new version (the text in force again saves nothing).
fn standing_save_route(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let body = match crate::rules::body_of(req) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(text) = body.get("text").and_then(Value::as_str) else {
        return Response::json(
            400,
            &json!({ "error": "text: the standing rules as a string" }),
        );
    };
    match app.standing_rules.save(text, "owner", apz_plaza::now_ms()) {
        Ok(_) => {
            let saved = crate::store::saver::save_now(app, "standing_rules");
            Response::json(200, &view(app, Some(saved)))
        }
        Err(why) => Response::json(400, &json!({ "error": why })),
    }
}

/// The document: the text in force, the version it is, and its history. `saved` is there when a save asked for it.
fn view(app: &App, saved: Option<bool>) -> Value {
    let record = app.standing_rules.snapshot();
    let current = record.versions.last();
    let mut body = json!({
        "text": current.map_or("", |v| v.text.as_str()),
        "version": current.map(|v| v.seq),
        "author": current.map(|v| v.author.as_str()),
        "updated_ms": current.map(|v| v.at_ms),
        "versions": record
            .versions
            .iter()
            .map(|v| json!({ "version": v.seq, "author": v.author, "at_ms": v.at_ms }))
            .collect::<Vec<_>>(),
    });
    if let Some(saved) = saved {
        body["saved"] = json!(saved);
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh record, loaded from a database that holds what `saved` holds.
    fn restored(saved: &StandingRules) -> StandingRules {
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &saved.rows().unwrap()).unwrap();
        let fresh = StandingRules::default();
        fresh.load(&conn).unwrap();
        fresh
    }

    #[test]
    fn each_save_is_a_version_and_the_newest_is_the_text_in_force() {
        let rules = StandingRules::default();
        assert_eq!(rules.text(), "");
        assert_eq!(rules.save("Never merge PRs.", "owner", 1), Ok(Some(1)));
        assert_eq!(
            rules.save("Never merge PRs; the owner merges.", "owner", 2),
            Ok(Some(2))
        );
        assert_eq!(rules.text(), "Never merge PRs; the owner merges.");
        let versions = rules.snapshot().versions;
        assert_eq!(versions.len(), 2);
        assert_eq!((versions[0].seq, versions[0].at_ms), (1, 1));
        assert_eq!(versions[0].text, "Never merge PRs.");
        assert_eq!(versions[1].author, "owner");
    }

    #[test]
    fn the_text_in_force_again_saves_nothing_and_the_text_is_trimmed() {
        let rules = StandingRules::default();
        assert_eq!(
            rules.save("Write plain sentences.", "owner", 1),
            Ok(Some(1))
        );
        assert_eq!(
            rules.save("  Write plain sentences.\n", "owner", 2),
            Ok(None)
        );
        assert_eq!(rules.snapshot().versions.len(), 1);
        assert_eq!(
            rules.save("  Write plain sentences.\n", "owner", 3),
            Ok(None)
        );
        assert_eq!(rules.snapshot().versions[0].text, "Write plain sentences.");
    }

    #[test]
    fn a_text_over_the_limit_is_refused_and_nothing_is_saved() {
        let rules = StandingRules::default();
        let long = "x".repeat(MAX_TEXT + 1);
        assert_eq!(
            rules.save(&long, "owner", 1),
            Err(format!("at most {MAX_TEXT} characters"))
        );
        assert!(rules.snapshot().versions.is_empty());
        assert_eq!(rules.save(&"x".repeat(MAX_TEXT), "owner", 2), Ok(Some(1)));
    }

    #[test]
    fn the_house_rules_become_the_first_version_once() {
        let rules = StandingRules::default();
        assert!(!rules.absorb("   ", 1), "nothing to absorb");
        assert!(rules.absorb("Ask before pushing.\n", 2));
        assert!(
            !rules.absorb("Something else.", 3),
            "the second start keeps the record"
        );
        let versions = rules.snapshot().versions;
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].text, "Ask before pushing.");
        assert_eq!(versions[0].author, "house rules");
        // A save after the absorption is the next version, and the house rules stay in the history.
        assert_eq!(
            rules.save("Ask before pushing; never force-push.", "owner", 4),
            Ok(Some(2))
        );
        assert_eq!(rules.snapshot().versions[0].text, "Ask before pushing.");
    }

    #[test]
    fn the_versions_survive_a_restart() {
        let rules = StandingRules::default();
        rules.save("First.", "owner", 1).unwrap();
        rules.save("Second.", "owner", 2).unwrap();
        assert_eq!(restored(&rules).snapshot(), rules.snapshot());
        assert_eq!(restored(&rules).text(), "Second.");
    }
}
