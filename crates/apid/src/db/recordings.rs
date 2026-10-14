//! The workstation recordings' index and their actions (stations/recordings.rs): `station_recordings` and
//! `station_recording_events` (migration 0010), or the state directory's files without a database (`db::files`).
//! The frames are not here: they are objects in the object store.

use postgres::Row;

use super::{Db, sql};
use crate::stations::recordings::{Event, Recording};

const NO_STORE: &str = "no database and no state directory for the workstation recordings";

/// One index row, in the column order of the recording queries (`station_recording_get.sql`).
fn recording_of(row: &Row) -> Recording {
    let opt = |i: usize| row.get::<_, Option<i64>>(i).map(|v| v as u64);
    Recording {
        id: row.get(0),
        station: row.get(1),
        lease: row.get(2),
        agent: row.get::<_, i64>(3) as u64,
        task: row.get(4),
        started_ms: row.get::<_, i64>(5) as u64,
        ended_ms: opt(6),
        frames: row.get::<_, i64>(7) as u64,
        actions: row.get::<_, i64>(8) as u64,
        bytes: row.get::<_, i64>(9) as u64,
        takeover: row.get(10),
        width: opt(11),
        height: opt(12),
    }
}

impl Db {
    /// Writes a recording's index row: a new one at its start, then the same id with its end and counts.
    pub fn put_recording(&self, recording: &Recording) -> Result<(), String> {
        if let Some(files) = self.file_store() {
            return files.put_recording(recording);
        }
        if !self.enabled() {
            return Err(NO_STORE.into());
        }
        let r = recording;
        let agent = r.agent as i64;
        let started = r.started_ms as i64;
        let ended = r.ended_ms.map(|v| v as i64);
        let (frames, actions, bytes) = (r.frames as i64, r.actions as i64, r.bytes as i64);
        let (width, height) = (r.width.map(|v| v as i64), r.height.map(|v| v as i64));
        self.run(|c| {
            c.execute(
                sql::STATION_RECORDING_PUT,
                &[&r.id, &r.station, &r.lease, &agent, &r.task, &started, &ended, &frames, &actions, &bytes, &r.takeover, &width, &height],
            )
        })
        .map(|_| ())
    }

    /// One recording's index row, or None when there is none.
    pub fn recording(&self, id: &str) -> Result<Option<Recording>, String> {
        if let Some(files) = self.file_store() {
            return files.recording(id);
        }
        if !self.enabled() {
            return Err(NO_STORE.into());
        }
        self.run(|c| c.query_opt(sql::STATION_RECORDING_GET, &[&id]))
            .map(|row| row.as_ref().map(recording_of))
            .inspect_err(|e| eprintln!("db: recording read failed: {e}"))
    }

    /// Recordings, newest first: one workstation's when `station` is given, only those started before `before` (ms)
    /// when it is given.
    pub fn recordings(&self, station: Option<&str>, before: Option<u64>, limit: i64) -> Result<Vec<Recording>, String> {
        if let Some(files) = self.file_store() {
            return files.recordings(station, before, limit);
        }
        if !self.enabled() {
            return Err(NO_STORE.into());
        }
        let before = before.map(|b| b as i64);
        self.run(|c| c.query(sql::STATION_RECORDING_PAGE, &[&station, &before, &limit]))
            .map(|rows| rows.iter().map(recording_of).collect())
            .inspect_err(|e| eprintln!("db: recording list failed: {e}"))
    }

    /// The recordings of one workstation that ended before `cutoff` (or never ended, and started before it), oldest
    /// first.
    pub fn recordings_expired(&self, station: &str, cutoff: u64) -> Result<Vec<Recording>, String> {
        if let Some(files) = self.file_store() {
            return files.recordings_expired(station, cutoff);
        }
        if !self.enabled() {
            return Err(NO_STORE.into());
        }
        let cutoff = cutoff as i64;
        self.run(|c| c.query(sql::STATION_RECORDING_EXPIRED, &[&station, &cutoff]))
            .map(|rows| rows.iter().map(recording_of).collect())
            .inspect_err(|e| eprintln!("db: recording sweep read failed: {e}"))
    }

    /// Deletes a recording's index row and, with it, its actions.
    pub fn delete_recording(&self, id: &str) -> Result<(), String> {
        if let Some(files) = self.file_store() {
            return files.delete_recording(id);
        }
        if !self.enabled() {
            return Err(NO_STORE.into());
        }
        self.run(|c| c.execute(sql::STATION_RECORDING_DELETE, &[&id])).map(|_| ())
    }

    /// Appends one action to a recording (table `station_recording_events`, or the file fallback).
    pub fn append_recording_event(&self, recording: &str, event: &Event) -> Result<(), String> {
        if let Some(files) = self.file_store() {
            return files.append_recording_event(recording, event);
        }
        if !self.enabled() {
            return Err(NO_STORE.into());
        }
        let at_ms = event.at_ms as i64;
        self.run(|c| c.execute(sql::STATION_RECORDING_EVENT_APPEND, &[&recording, &at_ms, &event.kind, &event.ok, &event.summary]))
            .map(|_| ())
    }

    /// The newest `limit` actions of a recording, oldest first.
    pub fn recording_events(&self, recording: &str, limit: i64) -> Result<Vec<Event>, String> {
        if let Some(files) = self.file_store() {
            return files.recording_events(recording, limit);
        }
        if !self.enabled() {
            return Err(NO_STORE.into());
        }
        self.run(|c| c.query(sql::STATION_RECORDING_EVENTS_PAGE, &[&recording, &limit]))
            .map(|rows| {
                rows.iter().map(|r| Event { at_ms: r.get::<_, i64>(0) as u64, kind: r.get(1), ok: r.get(2), summary: r.get(3) }).collect()
            })
            .inspect_err(|e| eprintln!("db: recording events read failed: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_database;
    use serde_json::json;

    fn recording(id: &str, station: &str, started_ms: u64, ended_ms: Option<u64>) -> Recording {
        Recording {
            id: id.into(),
            station: station.into(),
            lease: format!("l-{id}"),
            agent: 7,
            task: Some("render".into()),
            started_ms,
            ended_ms,
            frames: 0,
            actions: 0,
            bytes: 0,
            takeover: false,
            width: None,
            height: None,
        }
    }

    fn event(at_ms: u64, kind: &str) -> Event {
        Event { at_ms, kind: kind.into(), ok: true, summary: json!({ "kind": kind, "n": at_ms }) }
    }

    /// The same answers from both backends: the index's order and filters, the update of a row, the events' order
    /// and the cascade of a delete.
    fn check_recordings(db: &Db) {
        let (r1, r2, r3) = (recording("r1", "w1", 100, None), recording("r2", "w1", 200, None), recording("r3", "w2", 150, Some(300)));
        for row in [&r1, &r2, &r3] {
            db.put_recording(row).unwrap();
        }
        assert_eq!(db.recording("r1").unwrap(), Some(r1.clone()));
        assert_eq!(db.recording("nope").unwrap(), None);
        assert_eq!(db.recordings(None, None, 10).unwrap(), vec![r2.clone(), r3.clone(), r1.clone()], "newest start first");
        assert_eq!(db.recordings(Some("w1"), None, 10).unwrap(), vec![r2.clone(), r1.clone()]);
        assert_eq!(db.recordings(None, Some(200), 10).unwrap(), vec![r3.clone(), r1.clone()], "started before the cursor");
        assert_eq!(db.recordings(None, None, 1).unwrap(), vec![r2.clone()]);
        assert!(db.recordings(Some("w9"), None, 10).unwrap().is_empty());

        let ended = Recording {
            ended_ms: Some(150),
            frames: 3,
            actions: 2,
            bytes: 90,
            takeover: true,
            width: Some(960),
            height: Some(540),
            ..r1.clone()
        };
        db.put_recording(&ended).unwrap();
        assert_eq!(db.recording("r1").unwrap(), Some(ended.clone()), "the same id is updated, not added");
        assert_eq!(db.recordings(Some("w1"), None, 10).unwrap().len(), 2);

        for (at, kind) in [(10, "click"), (11, "type"), (12, "wait")] {
            db.append_recording_event("r1", &event(at, kind)).unwrap();
        }
        db.append_recording_event("r2", &event(13, "other")).unwrap();
        let kinds = |rows: Vec<Event>| rows.into_iter().map(|e| e.kind).collect::<Vec<_>>();
        assert_eq!(kinds(db.recording_events("r1", 10).unwrap()), vec!["click", "type", "wait"], "oldest first");
        assert_eq!(kinds(db.recording_events("r1", 2).unwrap()), vec!["type", "wait"], "the newest two");
        assert!(db.recording_events("r1", 0).unwrap().is_empty());
        assert_eq!(db.recording_events("r1", 10).unwrap()[0].summary, json!({ "kind": "click", "n": 10 }));

        // Expired: ended before the cutoff, or open and started before it. r2 is open since 200.
        let expired = |station: &str, cutoff: u64| db.recordings_expired(station, cutoff).unwrap();
        assert!(expired("w1", 150).is_empty(), "r1 ended at 150, not before it");
        assert_eq!(expired("w1", 160), vec![ended.clone()]);
        assert_eq!(expired("w1", 250), vec![ended.clone(), r2.clone()], "oldest start first");
        assert!(expired("w2", 300).is_empty(), "r3 ended at 300, not before it");

        db.delete_recording("r1").unwrap();
        assert_eq!(db.recording("r1").unwrap(), None);
        assert_eq!(db.recordings(None, None, 10).unwrap(), vec![r2.clone(), r3.clone()]);
        assert!(db.recording_events("r1", 10).unwrap().is_empty(), "a deleted recording's actions are gone");
        assert_eq!(kinds(db.recording_events("r2", 10).unwrap()), vec!["other"], "another's are kept");
    }

    /// Every recording query gives the same results on Postgres (a fresh test database, when the test server is set)
    /// and on the file fallback.
    #[test]
    fn recordings_behave_the_same_on_both_backends() {
        if let Some(url) = test_database("recordings_parity") {
            let env = std::collections::BTreeMap::from([("APZ_DATABASE_URL".to_string(), url)]);
            let db = Db::connect(&env);
            assert!(db.enabled());
            check_recordings(&db);
        }
        let dir = std::env::temp_dir().join(format!("apid-recordings-parity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let db = Db::files_only(&dir);
        assert!(!db.enabled());
        check_recordings(&db);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
