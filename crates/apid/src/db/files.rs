//! The file fallback for the audits and the recordings: used when apid runs without Postgres
//! (`APZ_DATABASE_URL` unset), with the same operations and orders as the Postgres queries in
//! `sql`. Append-only JSON lines, one record per line:
//!
//!   <state_dir>/ide-audit.jsonl                (newest) and ide-audit.1.jsonl, ide-audit.2.jsonl
//!   <state_dir>/station-audit.jsonl            (newest) and station-audit.1.jsonl, station-audit.2.jsonl
//!   <state_dir>/station-recordings.jsonl       (newest) and .1, .2: the recordings' index, a put or a delete per line
//!   <state_dir>/station-recording-events.jsonl (newest) and .1, .2: the recordings' actions
//!   <state_dir>/station-recording-frames.jsonl (newest) and .1, .2: the capture time of each frame
//!
//! The recordings' index is not append-only in Postgres (rows are updated and deleted), so here a line is a write:
//! the newest line of an id is its state, and a delete line ends it. Listing reads the whole index (it is small: a line
//! is a few hundred bytes), so a page costs the index's size, not the page's.
//!
//! An IDE audit line is the row as the route returns it (`{at_ms, actor, workspace, kind, ...}`), so a
//! file from before the audit moved into this store reads as it did. The audit file is private (mode
//! 0600), as the vault's audit log was.
//!
//! A file rotates to `.1` once it reaches `ROTATE_BYTES` (`.1` moves to `.2`; the old `.2` is
//! dropped). Rotation fsyncs the file it retires. Pages read backwards from the end of the files,
//! so a page costs its own size, not the file's. A line that does not parse (a write torn by a
//! crash) is skipped, and a new record never joins a torn line.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::stations::recordings::{Event, FrameTime, Recording};
use crate::stations::sync::Row;

/// The size at which a file rotates to its first generation.
pub const ROTATE_BYTES: u64 = 32 * 1024 * 1024;
/// The IDE audit's file in the state directory (the name the audit had before it moved into this store).
pub const IDE_AUDIT_FILE: &str = "ide-audit.jsonl";
/// The station audit's file in the state directory (stations/: owner actions and lease changes).
pub const STATION_AUDIT_FILE: &str = "station-audit.jsonl";
/// The recordings' index in the state directory (stations/recordings.rs).
pub const STATION_RECORDINGS_FILE: &str = "station-recordings.jsonl";
/// The recordings' actions in the state directory.
pub const RECORDING_EVENTS_FILE: &str = "station-recording-events.jsonl";
/// The capture times of the recordings' frames in the state directory.
pub const RECORDING_FRAMES_FILE: &str = "station-recording-frames.jsonl";
/// How much a backwards read takes from the file at a time.
const CHUNK: u64 = 64 * 1024;

/// One line of the recordings' index: the recording as written, or the deletion of an id.
#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum RecordingLine {
    Put { recording: Recording },
    Delete { id: String },
}

/// One line of the recordings' actions: the recording's id, then the event's fields.
#[derive(Serialize)]
struct EventOut<'a> {
    recording: &'a str,
    #[serde(flatten)]
    event: &'a Event,
}

#[derive(Deserialize)]
struct EventIn {
    recording: String,
    #[serde(flatten)]
    event: Event,
}

/// One line of the recordings' frames: the recording's id, then the frame's seq, capture time and size.
#[derive(Serialize)]
struct FrameOut {
    recording: String,
    seq: u64,
    ts_ms: u64,
    bytes: u64,
}

#[derive(Deserialize)]
struct FrameIn {
    recording: String,
    seq: u64,
    ts_ms: u64,
}

/// The open files, under one lock.
#[derive(Default)]
struct Open {
    ide_audit: Option<File>,
    station_audit: Option<File>,
    station_recordings: Option<File>,
    recording_events: Option<File>,
    recording_frames: Option<File>,
}

pub struct Files {
    dir: PathBuf,
    rotate_bytes: u64,
    chunk: u64,
    open: Mutex<Open>,
}

impl Files {
    /// The file store under `state_dir` (no file is touched until a record is written or read).
    pub fn open(state_dir: &Path) -> Files {
        Files::open_with(state_dir, ROTATE_BYTES, CHUNK)
    }

    /// `open` with the rotation size and the read block size given (tests use small ones).
    pub fn open_with(state_dir: &Path, rotate_bytes: u64, chunk: u64) -> Files {
        Files {
            dir: state_dir.to_path_buf(),
            rotate_bytes,
            chunk,
            open: Mutex::default(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Open> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn ide_audit_path(&self) -> PathBuf {
        self.dir.join(IDE_AUDIT_FILE)
    }

    fn station_audit_path(&self) -> PathBuf {
        self.dir.join(STATION_AUDIT_FILE)
    }

    fn recordings_path(&self) -> PathBuf {
        self.dir.join(STATION_RECORDINGS_FILE)
    }

    fn recording_events_path(&self) -> PathBuf {
        self.dir.join(RECORDING_EVENTS_FILE)
    }

    fn recording_frames_path(&self) -> PathBuf {
        self.dir.join(RECORDING_FRAMES_FILE)
    }

    /// Appends one IDE audit row and fsyncs it before returning: an owner action is on disk when its answer is.
    pub fn append_ide_audit(&self, row: &Value) -> Result<(), String> {
        let mut guard = self.lock();
        let open = &mut *guard;
        self.append_audit(&mut open.ide_audit, &self.ide_audit_path(), row)
    }

    /// IDE audit rows, newest first (by `at_ms`, then the file's order: the same as the table's order),
    /// of one workspace when `workspace` is given.
    pub fn ide_audit(&self, workspace: Option<&str>, limit: i64) -> Result<Vec<Value>, String> {
        let _guard = self.lock();
        self.audit_rows(&self.ide_audit_path(), "workspace", workspace, limit)
    }

    /// Appends one station audit row and fsyncs it (the same contract as the IDE audit's).
    pub fn append_station_audit(&self, row: &Value) -> Result<(), String> {
        let mut guard = self.lock();
        let open = &mut *guard;
        self.append_audit(&mut open.station_audit, &self.station_audit_path(), row)
    }

    /// Station audit rows, newest first, of one workstation when `station` is given.
    pub fn station_audit(&self, station: Option<&str>, limit: i64) -> Result<Vec<Value>, String> {
        let _guard = self.lock();
        self.audit_rows(&self.station_audit_path(), "station", station, limit)
    }

    /// Writes one index line of the recordings: a recording as it stands (its start, or its end and counts).
    pub fn put_recording(&self, recording: &Recording) -> Result<(), String> {
        let line = RecordingLine::Put {
            recording: recording.clone(),
        };
        let mut guard = self.lock();
        let open = &mut *guard;
        self.append_audit(&mut open.station_recordings, &self.recordings_path(), &line)
    }

    /// Writes the deletion of a recording's index line (its events stay in their file until it rotates out).
    pub fn delete_recording(&self, id: &str) -> Result<(), String> {
        let line = RecordingLine::Delete { id: id.to_string() };
        let mut guard = self.lock();
        let open = &mut *guard;
        self.append_audit(&mut open.station_recordings, &self.recordings_path(), &line)
    }

    /// One recording as the index holds it now, or None when it was never written or was deleted.
    pub fn recording(&self, id: &str) -> Result<Option<Recording>, String> {
        let _guard = self.lock();
        self.recording_unlocked(id).map_err(text)
    }

    /// `recording` for a caller that holds the lock: the newest line of `id` decides, so the scan stops there.
    fn recording_unlocked(&self, id: &str) -> io::Result<Option<Recording>> {
        let mut answer = None;
        let mut settled = false;
        for file in generations(&self.recordings_path()) {
            if settled {
                break;
            }
            for_each_line_rev(&file, self.chunk, |line| {
                match serde_json::from_slice::<RecordingLine>(line) {
                    Ok(RecordingLine::Put { recording }) if recording.id == id => {
                        answer = Some(recording);
                        settled = true;
                        false
                    }
                    Ok(RecordingLine::Delete { id: gone }) if gone == id => {
                        settled = true;
                        false
                    }
                    _ => true,
                }
            })?;
        }
        Ok(answer)
    }

    /// The state of every recording the index holds (the newest line of each id), by id; deleted ones are left out.
    fn recordings_latest(&self) -> io::Result<Vec<Recording>> {
        let mut seen: BTreeMap<String, Option<Recording>> = BTreeMap::new();
        for file in generations(&self.recordings_path()) {
            for_each_line_rev(&file, self.chunk, |line| {
                match serde_json::from_slice::<RecordingLine>(line) {
                    Ok(RecordingLine::Put { recording }) => {
                        seen.entry(recording.id.clone()).or_insert(Some(recording));
                    }
                    Ok(RecordingLine::Delete { id }) => {
                        seen.entry(id).or_insert(None);
                    }
                    Err(_) => {}
                }
                true
            })?;
        }
        Ok(seen.into_values().flatten().collect())
    }

    /// Recordings, newest first (by start, then id): one workstation's when `station` is given, and only those
    /// started before `before` (ms) when it is given.
    pub fn recordings(
        &self,
        station: Option<&str>,
        before: Option<u64>,
        limit: i64,
    ) -> Result<Vec<Recording>, String> {
        let limit = limit.max(0) as usize;
        let _guard = self.lock();
        let mut rows = self.recordings_latest().map_err(text)?;
        rows.retain(|r| {
            station.is_none_or(|wanted| r.station == wanted)
                && before.is_none_or(|b| r.started_ms < b)
        });
        rows.sort_by(|a, b| (b.started_ms, &b.id).cmp(&(a.started_ms, &a.id)));
        rows.truncate(limit);
        Ok(rows)
    }

    /// The recordings of one workstation that ended before `cutoff` (or never ended and started before it), oldest
    /// first: the retention sweep's list.
    pub fn recordings_expired(&self, station: &str, cutoff: u64) -> Result<Vec<Recording>, String> {
        let _guard = self.lock();
        let mut rows = self.recordings_latest().map_err(text)?;
        rows.retain(|r| r.station == station && r.ended_ms.unwrap_or(r.started_ms) < cutoff);
        rows.sort_by(|a, b| (a.started_ms, &a.id).cmp(&(b.started_ms, &b.id)));
        Ok(rows)
    }

    /// Appends one action to a recording's events and fsyncs it (the same contract as the audits).
    pub fn append_recording_event(&self, recording: &str, event: &Event) -> Result<(), String> {
        let line = EventOut { recording, event };
        let mut guard = self.lock();
        let open = &mut *guard;
        self.append_audit(
            &mut open.recording_events,
            &self.recording_events_path(),
            &line,
        )
    }

    /// Appends one frame's capture time to the recordings' frames, and fsyncs it.
    pub fn put_recording_frame(
        &self,
        recording: &str,
        frame: FrameTime,
        bytes: u64,
    ) -> Result<(), String> {
        let line = FrameOut {
            recording: recording.to_string(),
            seq: frame.seq,
            ts_ms: frame.ts_ms,
            bytes,
        };
        let mut guard = self.lock();
        let open = &mut *guard;
        self.append_audit(
            &mut open.recording_frames,
            &self.recording_frames_path(),
            &line,
        )
    }

    /// The capture times of a recording's frames, in seq order: those with a seq up to `upto` (the frames its index
    /// counts), at most `limit`. Nothing when the recording is not in the index. A seq written twice keeps its last
    /// time, as the Postgres upsert does.
    pub fn recording_frame_times(
        &self,
        recording: &str,
        upto: u64,
        limit: i64,
    ) -> Result<Vec<FrameTime>, String> {
        let limit = limit.max(0) as usize;
        let _guard = self.lock();
        if self.recording_unlocked(recording).map_err(text)?.is_none() {
            return Ok(Vec::new());
        }
        // Newest first, so the first line of a seq is its last write; the seq order then comes from the map.
        let mut times: BTreeMap<u64, u64> = BTreeMap::new();
        for file in generations(&self.recording_frames_path()) {
            for_each_line_rev(&file, self.chunk, |line| {
                if let Some(row) = serde_json::from_slice::<FrameIn>(line)
                    .ok()
                    .filter(|row| row.recording == recording && row.seq <= upto)
                {
                    times.entry(row.seq).or_insert(row.ts_ms);
                }
                true
            })
            .map_err(text)?;
        }
        Ok(times
            .into_iter()
            .take(limit)
            .map(|(seq, ts_ms)| FrameTime { seq, ts_ms })
            .collect())
    }

    /// The newest `limit` actions of a recording, oldest first. Nothing when the recording is not in the index (a
    /// deleted one's events are unreachable, as Postgres' cascade makes them).
    pub fn recording_events(&self, recording: &str, limit: i64) -> Result<Vec<Event>, String> {
        let limit = limit.max(0) as usize;
        let _guard = self.lock();
        if self.recording_unlocked(recording).map_err(text)?.is_none() {
            return Ok(Vec::new());
        }
        let mut rows: Vec<Event> = Vec::new();
        for file in generations(&self.recording_events_path()) {
            if rows.len() >= limit {
                break;
            }
            for_each_line_rev(&file, self.chunk, |line| {
                if let Ok(row) = serde_json::from_slice::<EventIn>(line)
                    && row.recording == recording
                {
                    rows.push(row.event);
                }
                rows.len() < limit
            })
            .map_err(text)?;
        }
        rows.truncate(limit);
        rows.reverse();
        Ok(rows)
    }

    /// The work-folder sync rows of one workstation: `<dir>/station-sync/<station>.json`, a whole file rewritten on each
    /// save (the rows are few, and a path's row is replaced, so no line log is needed).
    pub fn station_sync_rows(&self, station: &str) -> Result<Vec<Row>, String> {
        let _guard = self.lock();
        read_sync_rows(&self.sync_path(station)?).map_err(text)
    }

    /// Replaces the sync rows named in `rows`, keeping the others of the workstation.
    pub fn save_station_sync_rows(&self, station: &str, rows: &[Row]) -> Result<(), String> {
        let _guard = self.lock();
        let path = self.sync_path(station)?;
        let mut all: BTreeMap<String, Row> = read_sync_rows(&path)
            .map_err(text)?
            .into_iter()
            .map(|r| (r.path.clone(), r))
            .collect();
        for row in rows {
            all.insert(row.path.clone(), row.clone());
        }
        let list: Vec<&Row> = all.values().collect();
        let bytes = serde_json::to_vec_pretty(&list).map_err(text)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(text)?;
        }
        crate::providers::write_atomic(&path, &bytes, 0o600).map_err(text)
    }

    /// The file of one workstation's sync rows. The id must be plain, since it names the file.
    fn sync_path(&self, station: &str) -> Result<PathBuf, String> {
        let plain = !station.is_empty()
            && station
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_');
        if !plain {
            return Err(format!("{station:?} is not a workstation id"));
        }
        Ok(self
            .dir
            .join("station-sync")
            .join(format!("{station}.json")))
    }

    /// Appends one audit row to `path` (private, mode 0600) and fsyncs it. The caller holds the lock.
    fn append_audit<T: Serialize>(
        &self,
        slot: &mut Option<File>,
        path: &Path,
        row: &T,
    ) -> Result<(), String> {
        let line = to_line(row)?;
        self.append_line(slot, path, &line, true).map_err(text)?;
        match slot.as_ref() {
            Some(file) => file.sync_data().map_err(text),
            None => Ok(()),
        }
    }

    /// Audit rows of `path`, newest first (a stable sort by `at_ms` keeps the file's order among ties),
    /// filtered to those whose `column` equals `value` when one is given. The caller holds the lock.
    fn audit_rows(
        &self,
        path: &Path,
        column: &str,
        value: Option<&str>,
        limit: i64,
    ) -> Result<Vec<Value>, String> {
        let limit = limit.max(0) as usize;
        let mut rows: Vec<Value> = Vec::new();
        for file in generations(path) {
            if rows.len() >= limit {
                break;
            }
            for_each_line_rev(&file, self.chunk, |line| {
                let row = serde_json::from_slice::<Value>(line)
                    .ok()
                    .filter(Value::is_object);
                if let Some(row) =
                    row.filter(|row| value.is_none_or(|wanted| row[column] == wanted))
                {
                    rows.push(row);
                }
                rows.len() < limit
            })
            .map_err(text)?;
        }
        rows.sort_by_key(|row| std::cmp::Reverse(row["at_ms"].as_u64().unwrap_or(0)));
        rows.truncate(limit);
        Ok(rows)
    }

    /// Writes one line to `path` through `slot` (the open file), rotating first when the file has
    /// reached the limit. A `private` file is created mode 0600.
    fn append_line(
        &self,
        slot: &mut Option<File>,
        path: &Path,
        line: &[u8],
        private: bool,
    ) -> io::Result<()> {
        let mut file = match slot.take() {
            Some(file) => file,
            None => open_for_append(path, private)?,
        };
        if file.metadata()?.len() >= self.rotate_bytes {
            drop(file);
            rotate(path)?;
            file = open_for_append(path, private)?;
        }
        file.write_all(line)?;
        *slot = Some(file);
        Ok(())
    }
}

/// One workstation's sync rows from its file; a missing file holds none.
fn read_sync_rows(path: &Path) -> io::Result<Vec<Row>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

fn text(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn to_line<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let mut line = serde_json::to_vec(value).map_err(text)?;
    line.push(b'\n');
    Ok(line)
}

/// `<stem>.<n>.jsonl` next to `path` (`123.jsonl` gives `123.1.jsonl`).
fn generation(path: &Path, n: u32) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    path.with_file_name(format!("{stem}.{n}.jsonl"))
}

/// The live file, then its two rotations, newest first.
fn generations(path: &Path) -> [PathBuf; 3] {
    [path.to_path_buf(), generation(path, 1), generation(path, 2)]
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Opens `path` for appending (creating it and its folder; mode 0600 when `private`). A last line
/// without its newline (a torn write) gets one first, so the next record starts on its own line.
fn open_for_append(path: &Path, private: bool) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true).read(true);
    if private {
        private_mode(&mut options);
    }
    let mut file = options.open(path)?;
    if file.metadata()?.len() > 0 {
        file.seek(SeekFrom::End(-1))?;
        let mut last = [0u8; 1];
        file.read_exact(&mut last)?;
        if last[0] != b'\n' {
            file.write_all(b"\n")?;
        }
    }
    Ok(file)
}

#[cfg(unix)]
fn private_mode(options: &mut OpenOptions) {
    std::os::unix::fs::OpenOptionsExt::mode(options, 0o600);
}

/// No mode bits on other systems: a file under the user's profile carries the profile's ACL.
#[cfg(not(unix))]
fn private_mode(_options: &mut OpenOptions) {}

/// Retires `path` to `.1` (and `.1` to `.2`, dropping the old `.2`), fsyncing it and the folder.
/// The caller has closed its handle to `path` (`append_line` takes and drops it first), so the
/// renames do not meet a file that Windows holds open.
fn rotate(path: &Path) -> io::Result<()> {
    sync_file(path)?;
    remove_if_exists(&generation(path, 2))?;
    let first = generation(path, 1);
    if first.exists() {
        fs::rename(&first, generation(path, 2))?;
    }
    fs::rename(path, &first)?;
    sync_folder(path.parent())
}

/// Flushes `path` to disk. Windows flushes a handle only if it can write, so the file is opened
/// for append (as the writers open it) rather than read-only.
fn sync_file(path: &Path) -> io::Result<()> {
    OpenOptions::new().append(true).open(path)?.sync_all()
}

/// Flushes a folder's entries to disk. Unix only: Windows cannot open a folder as a file (the open
/// is refused), and it has no folder fsync to make a rename durable.
#[cfg(unix)]
fn sync_folder(dir: Option<&Path>) -> io::Result<()> {
    if let Some(dir) = dir {
        File::open(dir)?.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn sync_folder(_dir: Option<&Path>) -> io::Result<()> {
    Ok(())
}

/// Calls `visit` with each complete line of `path`, newest first, until it returns false. Reads
/// `chunk` bytes at a time from the end; a line split across blocks is joined before it is visited.
fn for_each_line_rev(
    path: &Path,
    chunk: u64,
    mut visit: impl FnMut(&[u8]) -> bool,
) -> io::Result<()> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let mut pos = file.metadata()?.len();
    // The bytes read so far that do not yet end in a newline boundary we have passed.
    let mut tail: Vec<u8> = Vec::new();
    loop {
        while let Some(newline) = tail.iter().rposition(|b| *b == b'\n') {
            let keep_going = visit(&tail[newline + 1..]);
            tail.truncate(newline);
            if !keep_going {
                return Ok(());
            }
        }
        if pos == 0 {
            if !tail.is_empty() {
                visit(&tail);
            }
            return Ok(());
        }
        let step = chunk.max(1).min(pos);
        pos -= step;
        let mut block = vec![0u8; step as usize];
        file.seek(SeekFrom::Start(pos))?;
        file.read_exact(&mut block)?;
        block.extend_from_slice(&tail);
        tail = block;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A fresh folder for one test's state directory.
    fn state(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apid-files-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("state dir");
        dir
    }

    /// The IDE audit: rows come back newest first (by at_ms, then the later row first), filtered by
    /// workspace and cut to the limit, across a restart. The file is private.
    #[test]
    fn ide_audit_round_trips_filters_limits_and_pages_newest_first() {
        let dir = state("ide-audit");
        let row = |at_ms: u64, workspace: &str, kind: &str| json!({ "at_ms": at_ms, "actor": "owner", "workspace": workspace, "kind": kind, "path": null });
        let kinds = |rows: Vec<Value>| {
            rows.iter()
                .map(|r| r["kind"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        };

        let files = Files::open(&dir);
        files
            .append_ide_audit(&row(10, "agent:12", "write"))
            .unwrap();
        files
            .append_ide_audit(&row(20, "agent:40", "write"))
            .unwrap();
        files
            .append_ide_audit(&row(20, "agent:12", "restore"))
            .unwrap();
        drop(files);

        let files = Files::open(&dir);
        files.append_ide_audit(&row(30, "agent:12", "run")).unwrap();
        assert_eq!(
            kinds(files.ide_audit(None, 10).unwrap()),
            vec!["run", "restore", "write", "write"]
        );
        assert_eq!(
            kinds(files.ide_audit(Some("agent:12"), 10).unwrap()),
            vec!["run", "restore", "write"]
        );
        assert_eq!(
            kinds(files.ide_audit(Some("agent:12"), 2).unwrap()),
            vec!["run", "restore"]
        );
        assert_eq!(
            kinds(files.ide_audit(Some("agent:40"), 10).unwrap()),
            vec!["write"]
        );
        assert!(files.ide_audit(Some("agent:99"), 10).unwrap().is_empty());
        assert_eq!(
            files.ide_audit(None, 1).unwrap(),
            vec![row(30, "agent:12", "run")]
        );
        assert!(files.ide_audit(None, 0).unwrap().is_empty());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.join("ide-audit.jsonl"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "the audit file is private");
        }
    }

    /// The station audit: newest first, filtered by station (a registry row has a null station, and is listed only
    /// in the whole), kept across a restart, and written to its own file.
    #[test]
    fn station_audit_round_trips_filters_and_pages_newest_first() {
        let dir = state("station-audit");
        let files = Files::open(&dir);
        let row = |at_ms: u64, station: Option<&str>, kind: &str| json!({ "at_ms": at_ms, "actor": "owner", "station": station, "agent": null, "kind": kind });
        let kinds = |rows: Vec<Value>| {
            rows.iter()
                .map(|r| r["kind"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        };
        files
            .append_station_audit(&row(10, Some("w1"), "join"))
            .unwrap();
        files
            .append_station_audit(&row(20, Some("w2"), "join"))
            .unwrap();
        files
            .append_station_audit(&row(20, None, "code_issue"))
            .unwrap();
        files
            .append_station_audit(&row(30, Some("w1"), "drain"))
            .unwrap();
        assert_eq!(
            kinds(files.station_audit(None, 10).unwrap()),
            vec!["drain", "code_issue", "join", "join"]
        );
        assert_eq!(
            kinds(files.station_audit(Some("w1"), 10).unwrap()),
            vec!["drain", "join"]
        );
        assert_eq!(
            kinds(files.station_audit(Some("w1"), 1).unwrap()),
            vec!["drain"]
        );
        assert!(files.station_audit(Some("w9"), 10).unwrap().is_empty());
        assert!(files.station_audit(None, 0).unwrap().is_empty());
        assert!(dir.join(STATION_AUDIT_FILE).exists());
        assert_eq!(
            files.ide_audit(None, 10).unwrap().len(),
            0,
            "the IDE audit's file is its own"
        );

        let reopened = Files::open(&dir);
        assert_eq!(
            kinds(reopened.station_audit(Some("w2"), 10).unwrap()),
            vec!["join"],
            "the rows survive a restart"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
