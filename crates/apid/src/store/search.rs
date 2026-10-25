//! Full-text search over the journal's notes and the project memory's entries (FTS5, the two virtual tables in
//! `sql/plaza/0001_init.sql`'s header).
//!
//! The index is a copy of the rows it indexes, rebuilt by the saver in the same transaction that writes those rows
//! ([`reindex`], from `saver::save_one`), so a search reads what is saved and a restart reads the same index. A query
//! matches whole words, every word of it, and the best match (bm25) comes first. A search that cannot read the index
//! is the caller's to answer in memory.

use apz_store::StoreError;
use apz_store::rusqlite::{Connection, params};

use super::sql;

/// Rebuilds the index of the component `component` (its name in `components.rs`) from its rows, when it has one.
/// Other components have none and are left alone.
pub fn reindex(conn: &Connection, component: &str) -> Result<(), StoreError> {
    let (clear, fill) = match component {
        "journal" => (sql::JOURNAL_SEARCH_CLEAR, sql::JOURNAL_SEARCH_FILL),
        "memory_entries" => (sql::MEMORY_SEARCH_CLEAR, sql::MEMORY_SEARCH_FILL),
        _ => return Ok(()),
    };
    conn.execute(clear, [])?;
    conn.execute(fill, [])?;
    Ok(())
}

/// The numbers of the notes of `task` that match `text`, best match first.
pub fn journal_notes(conn: &Connection, task: &str, text: &str) -> Result<Vec<u64>, StoreError> {
    let mut stmt = conn.prepare_cached(sql::JOURNAL_SEARCH_HITS)?;
    let seqs = stmt
        .query_map(params![match_expr(text), task], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<i64>, _>>()?;
    Ok(seqs.into_iter().map(|seq| seq as u64).collect())
}

/// The keys of the live entries of `project` that match `text`, and are of type `kind` and anchored to `anchor` when
/// those are given (empty means any), best match first.
pub fn memory_entries(
    conn: &Connection,
    project: &str,
    text: &str,
    kind: &str,
    anchor: &str,
) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare_cached(sql::MEMORY_SEARCH_HITS)?;
    let keys = stmt
        .query_map(params![match_expr(text), project, kind, anchor], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(keys)
}

/// The FTS5 expression for `text`: each of its words, quoted (so no word is read as syntax), which must all match.
fn match_expr(text: &str) -> String {
    text.split_whitespace()
        .map(|word| format!("\"{}\"", word.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{Journal, Kind, Note};
    use crate::project_memory::{Entry, entry_def};
    use apz_store::{RowSet, table};

    /// A plaza.db at its migrations, as the hub opens it.
    fn migrated(conn: &Connection) {
        // A test's file is never synced: the migration's statements each commit, and a sync apiece took minutes on a
        // shared CI disk.
        conn.pragma_update(None, "synchronous", "OFF").unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        for m in crate::store::PLAZA_MIGRATIONS {
            conn.execute_batch(m.sql).unwrap();
        }
    }

    fn note(seq: u64, text: &str, reason: Option<&str>) -> Note {
        Note {
            seq,
            kind: Kind::Decision,
            session: None,
            text: text.to_string(),
            reason: reason.map(str::to_string),
            thread: None,
            at_ms: 1,
        }
    }

    /// The journals as the saver writes them: `table::replace` puts their rows in plaza.db.
    fn store_journals(conn: &Connection, journals: &[Journal]) {
        let rows = RowSet::list(crate::journal::def(), journals.iter()).unwrap();
        table::replace(conn, crate::journal::def(), &rows).unwrap();
    }

    fn entry(id: &str, kind: &str, status: &str, title: &str, body: &str) -> Entry {
        Entry {
            key: format!("pr/{id}"),
            project: "pr".to_string(),
            id: id.to_string(),
            kind: kind.to_string(),
            status: status.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            path: format!(".plaza/memory/{id}.md"),
            anchors: Vec::new(),
            supersedes: Vec::new(),
            links: Vec::new(),
            verified_commit: String::new(),
            verified_date: String::new(),
            provenance_task: String::new(),
            provenance_session: String::new(),
            provenance_change: String::new(),
        }
    }

    fn store_entries(conn: &Connection, entries: &[Entry]) {
        let rows = RowSet::list(entry_def(), entries.iter()).unwrap();
        table::replace(conn, entry_def(), &rows).unwrap();
    }

    fn journal() -> Journal {
        Journal {
            task: "t1".to_string(),
            notes: vec![
                note(1, "retry the upload after the lock is released", None),
                note(
                    2,
                    "the lock lock lock is slow",
                    Some("lock contention on the lock"),
                ),
                note(3, "blockade of the gate", None),
            ],
            handovers: Vec::new(),
        }
    }

    #[test]
    fn a_note_is_found_by_its_words_and_the_better_match_comes_first() {
        let conn = Connection::open_in_memory().unwrap();
        migrated(&conn);
        store_journals(&conn, &[journal()]);
        reindex(&conn, "journal").unwrap();
        assert_eq!(journal_notes(&conn, "t1", "lock").unwrap(), [2, 1]);
        assert_eq!(journal_notes(&conn, "t1", "blockade").unwrap(), [3]);
        assert!(
            journal_notes(&conn, "t1", "loc").unwrap().is_empty(),
            "a word is matched whole, not as a part of another"
        );
        assert_eq!(journal_notes(&conn, "t1", "upload lock").unwrap(), [1]);
        assert!(
            journal_notes(&conn, "t2", "lock").unwrap().is_empty(),
            "another task's notes do not match"
        );
    }

    #[test]
    fn the_index_survives_a_restart_of_the_file() {
        let dir = std::env::temp_dir().join(format!("apid-fts-restart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plaza.db");
        {
            let conn = Connection::open(&path).unwrap();
            migrated(&conn);
            store_journals(&conn, &[journal()]);
            reindex(&conn, "journal").unwrap();
        }
        let reopened = Connection::open(&path).unwrap();
        let found = journal_notes(&reopened, "t1", "lock");
        drop(reopened);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(found.unwrap(), [2, 1]);
    }

    #[test]
    fn a_rebuild_follows_the_rows_and_a_memory_search_filters_by_status_type_and_anchor() {
        let conn = Connection::open_in_memory().unwrap();
        migrated(&conn);
        let mut mover = entry(
            "D-1",
            "decision",
            "active",
            "Lock order",
            "take the mover lock before the claim lock",
        );
        mover.anchors = vec!["src/mover.rs::claim".to_string()];
        let timeout = entry(
            "P-2",
            "pitfall",
            "flagged",
            "Lock timeout",
            "the lock times out after five seconds",
        );
        let retired = entry("P-3", "pitfall", "retired", "Old lock", "lock was removed");
        store_entries(&conn, &[mover.clone(), timeout.clone(), retired]);
        reindex(&conn, "memory_entries").unwrap();

        let live = memory_entries(&conn, "pr", "lock", "", "").unwrap();
        assert_eq!(live, ["pr/D-1", "pr/P-2"], "retired entries do not match");
        assert_eq!(
            memory_entries(&conn, "pr", "lock", "pitfall", "").unwrap(),
            ["pr/P-2"]
        );
        assert_eq!(
            memory_entries(&conn, "pr", "lock", "", "src/mover.rs::claim").unwrap(),
            ["pr/D-1"]
        );
        assert!(
            memory_entries(&conn, "other", "lock", "", "")
                .unwrap()
                .is_empty()
        );

        let mut edited = mover;
        edited.body = "take the claim lock first".to_string();
        store_entries(&conn, &[edited, timeout]);
        reindex(&conn, "memory_entries").unwrap();
        assert!(
            !memory_entries(&conn, "pr", "mover", "", "")
                .unwrap()
                .iter()
                .any(|k| k == "pr/D-1"),
            "the rebuild drops the words an entry no longer has"
        );
    }

    #[test]
    fn each_word_is_quoted_so_quotes_in_a_query_are_text() {
        assert_eq!(match_expr("say \"hi\""), "\"say\" \"\"\"hi\"\"\"");
        assert_eq!(match_expr("   "), "");
    }
}
