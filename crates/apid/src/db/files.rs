//! The file fallback for transcripts and infractions: used when apid runs without Postgres
//! (`APZ_DATABASE_URL` unset), with the same operations and orders as the Postgres queries in
//! `sql`. Append-only JSON lines, one record per line:
//!
//!   <state_dir>/transcripts/<cid>.jsonl        (newest) and <cid>.1.jsonl, <cid>.2.jsonl
//!   <state_dir>/infractions.jsonl              (newest) and infractions.1.jsonl, infractions.2.jsonl
//!
//! A file rotates to `.1` once it reaches `ROTATE_BYTES` (`.1` moves to `.2`; the old `.2` is
//! dropped). Rotation fsyncs the file it retires. Pages read backwards from the end of the files,
//! so a page costs its own size, not the file's. A line that does not parse (a write torn by a
//! crash) is skipped, and a new record never joins a torn line. A seq already stored for the cid
//! in a kept generation is ignored, as the primary key would ignore it; a seq above the cid's
//! highest stored seq is appended without a scan. Unlike Postgres, a record rotated out of the two
//! kept generations is forgotten, so a seq that old would be accepted again (the agents' seqs only
//! grow, so this does not arise).

use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Infraction;

/// The size at which a file rotates to its first generation.
pub const ROTATE_BYTES: u64 = 32 * 1024 * 1024;
/// How much a backwards read takes from the file at a time.
const CHUNK: u64 = 64 * 1024;

/// One transcript record as a page returns it: (seq, work, kind, at_ms, data).
pub type TranscriptRow = (u64, u64, String, u64, Value);

#[derive(Serialize)]
struct TranscriptOut<'a> {
    seq: u64,
    work: u64,
    kind: &'a str,
    at_ms: u64,
    data: &'a Value,
}

#[derive(Deserialize)]
struct TranscriptIn {
    seq: u64,
    work: u64,
    kind: String,
    at_ms: u64,
    data: Value,
}

#[derive(Serialize)]
struct InfractionOut<'a> {
    id: u64,
    cid: u64,
    law: &'a str,
    at_ms: u64,
    tool: &'a str,
    detail: &'a str,
}

#[derive(Deserialize)]
struct InfractionIn {
    id: u64,
    cid: u64,
    law: String,
    at_ms: u64,
    tool: String,
    detail: String,
}

/// The open files and the per-cid bookkeeping, under one lock.
#[derive(Default)]
struct Open {
    transcripts: HashMap<u64, Option<File>>,
    /// The highest seq stored per cid (None: nothing stored), read lazily from the tail.
    high: HashMap<u64, Option<u64>>,
    infractions: Option<File>,
    /// The next infraction id (read lazily from the tail; ids start at 1, as a bigserial does).
    next_id: Option<u64>,
    /// Appended since the last `sync`.
    dirty: bool,
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
        Files { dir: state_dir.to_path_buf(), rotate_bytes, chunk, open: Mutex::default() }
    }

    fn lock(&self) -> MutexGuard<'_, Open> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn transcript_path(&self, cid: u64) -> PathBuf {
        self.dir.join("transcripts").join(format!("{cid}.jsonl"))
    }

    fn infraction_path(&self) -> PathBuf {
        self.dir.join("infractions.jsonl")
    }

    /// Appends one transcript record. `Ok(false)` when `seq` is already stored for `cid`.
    pub fn append_transcript(&self, cid: u64, seq: u64, work: u64, kind: &str, at_ms: u64, data: &Value) -> Result<bool, String> {
        let mut guard = self.lock();
        let open = &mut *guard;
        let high = match open.high.get(&cid).copied() {
            Some(high) => high,
            None => {
                let high = self.newest(&self.transcript_path(cid), |r: &TranscriptIn| r.seq).map_err(text)?;
                open.high.insert(cid, high);
                high
            }
        };
        if high.is_some_and(|high| seq <= high) && self.has_transcript_seq(cid, seq).map_err(text)? {
            return Ok(false);
        }
        let line = to_line(&TranscriptOut { seq, work, kind, at_ms, data })?;
        let slot = open.transcripts.entry(cid).or_default();
        self.append_line(slot, &self.transcript_path(cid), &line).map_err(text)?;
        open.high.insert(cid, Some(high.map_or(seq, |high| high.max(seq))));
        open.dirty = true;
        Ok(true)
    }

    /// The newest `limit` records of `cid`, oldest first (the order `Db::transcript` returns).
    pub fn transcript(&self, cid: u64, limit: i64) -> Result<Vec<TranscriptRow>, String> {
        let limit = limit.max(0) as usize;
        let _guard = self.lock();
        let mut rows: Vec<TranscriptRow> = Vec::new();
        for file in generations(&self.transcript_path(cid)) {
            if rows.len() >= limit {
                break;
            }
            for_each_line_rev(&file, self.chunk, |line| {
                if let Ok(row) = serde_json::from_slice::<TranscriptIn>(line) {
                    rows.push((row.seq, row.work, row.kind, row.at_ms, row.data));
                }
                rows.len() < limit
            })
            .map_err(text)?;
        }
        rows.sort_by(|a, b| b.0.cmp(&a.0));
        rows.truncate(limit);
        rows.reverse();
        Ok(rows)
    }

    /// Every cid that has transcript records.
    pub fn transcript_cids(&self) -> Result<Vec<u64>, String> {
        let dir = self.dir.join("transcripts");
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.to_string()),
        };
        let mut cids = BTreeSet::new();
        for entry in entries {
            let name = entry.map_err(text)?.file_name();
            let name = name.to_string_lossy();
            if let Some(stem) = name.strip_suffix(".jsonl") {
                if let Some(cid) = stem.split('.').next().and_then(|cid| cid.parse::<u64>().ok()) {
                    cids.insert(cid);
                }
            }
        }
        Ok(cids.into_iter().collect())
    }

    /// Deletes every generation of `cid`'s transcript.
    pub fn delete_transcript(&self, cid: u64) -> Result<(), String> {
        let mut open = self.lock();
        open.transcripts.remove(&cid);
        open.high.remove(&cid);
        for file in generations(&self.transcript_path(cid)) {
            remove_if_exists(&file).map_err(text)?;
        }
        Ok(())
    }

    /// Appends one infraction, with the next id.
    pub fn append_infraction(&self, infraction: &Infraction) -> Result<(), String> {
        let mut guard = self.lock();
        let open = &mut *guard;
        let id = match open.next_id {
            Some(id) => id,
            None => self.newest(&self.infraction_path(), |r: &InfractionIn| r.id).map_err(text)?.map_or(1, |last| last + 1),
        };
        let line = to_line(&InfractionOut {
            id,
            cid: infraction.cid,
            law: &infraction.law,
            at_ms: infraction.at_ms,
            tool: &infraction.tool,
            detail: &infraction.detail,
        })?;
        self.append_line(&mut open.infractions, &self.infraction_path(), &line).map_err(text)?;
        open.next_id = Some(id + 1);
        open.dirty = true;
        Ok(())
    }

    /// Infractions, newest first (by `at_ms`, then id): one agent's (`Some(cid)`) or the plaza's.
    pub fn infractions(&self, cid: Option<u64>, limit: i64) -> Result<Vec<Infraction>, String> {
        let limit = limit.max(0) as usize;
        let _guard = self.lock();
        let mut rows: Vec<InfractionIn> = Vec::new();
        for file in generations(&self.infraction_path()) {
            if rows.len() >= limit {
                break;
            }
            for_each_line_rev(&file, self.chunk, |line| {
                if let Ok(row) = serde_json::from_slice::<InfractionIn>(line) {
                    if cid.map_or(true, |wanted| wanted == row.cid) {
                        rows.push(row);
                    }
                }
                rows.len() < limit
            })
            .map_err(text)?;
        }
        rows.sort_by(|a, b| (b.at_ms, b.id).cmp(&(a.at_ms, a.id)));
        rows.truncate(limit);
        Ok(rows
            .into_iter()
            .map(|r| Infraction { cid: r.cid, law: r.law, at_ms: r.at_ms, tool: r.tool, detail: r.detail })
            .collect())
    }

    /// Fsyncs the files written since the last call (the periodic saver runs it).
    pub fn sync(&self) -> Result<(), String> {
        let mut open = self.lock();
        if !open.dirty {
            return Ok(());
        }
        for file in open.transcripts.values().flatten().chain(open.infractions.iter()) {
            file.sync_all().map_err(text)?;
        }
        open.dirty = false;
        Ok(())
    }

    /// Whether `seq` is stored for `cid`, in any generation.
    fn has_transcript_seq(&self, cid: u64, seq: u64) -> io::Result<bool> {
        for file in generations(&self.transcript_path(cid)) {
            let mut found = false;
            for_each_line_rev(&file, self.chunk, |line| {
                found = serde_json::from_slice::<TranscriptIn>(line).is_ok_and(|row| row.seq == seq);
                !found
            })?;
            if found {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The key (seq or id) of the newest record in `path`'s generations, if any has one.
    fn newest<T: DeserializeOwned>(&self, path: &Path, key: impl Fn(&T) -> u64) -> io::Result<Option<u64>> {
        for file in generations(path) {
            let mut found = None;
            for_each_line_rev(&file, self.chunk, |line| match serde_json::from_slice::<T>(line) {
                Ok(row) => {
                    found = Some(key(&row));
                    false
                }
                Err(_) => true,
            })?;
            if found.is_some() {
                return Ok(found);
            }
        }
        Ok(None)
    }

    /// Writes one line to `path` through `slot` (the open file), rotating first when the file has
    /// reached the limit.
    fn append_line(&self, slot: &mut Option<File>, path: &Path, line: &[u8]) -> io::Result<()> {
        let mut file = match slot.take() {
            Some(file) => file,
            None => open_for_append(path)?,
        };
        if file.metadata()?.len() >= self.rotate_bytes {
            drop(file);
            rotate(path)?;
            file = open_for_append(path)?;
        }
        file.write_all(line)?;
        *slot = Some(file);
        Ok(())
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
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
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

/// Opens `path` for appending (creating it and its folder). A last line without its newline (a
/// torn write) gets one first, so the next record starts on its own line.
fn open_for_append(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).read(true).open(path)?;
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

/// Retires `path` to `.1` (and `.1` to `.2`, dropping the old `.2`), fsyncing it and the folder.
fn rotate(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()?;
    remove_if_exists(&generation(path, 2))?;
    let first = generation(path, 1);
    if first.exists() {
        fs::rename(&first, generation(path, 2))?;
    }
    fs::rename(path, &first)?;
    if let Some(dir) = path.parent() {
        File::open(dir)?.sync_all()?;
    }
    Ok(())
}

/// Calls `visit` with each complete line of `path`, newest first, until it returns false. Reads
/// `chunk` bytes at a time from the end; a line split across blocks is joined before it is visited.
fn for_each_line_rev(path: &Path, chunk: u64, mut visit: impl FnMut(&[u8]) -> bool) -> io::Result<()> {
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

    fn seqs(rows: &[TranscriptRow]) -> Vec<u64> {
        rows.iter().map(|r| r.0).collect()
    }

    #[test]
    fn reverse_reader_yields_whole_lines_newest_first_across_blocks() {
        let dir = state("reverse");
        let path = dir.join("lines.jsonl");
        let text = "first line\nsecond is longer than the block size\n\nlast";
        fs::write(&path, text).unwrap();
        for chunk in [1u64, 3, 7, 64] {
            let mut seen = Vec::new();
            for_each_line_rev(&path, chunk, |line| {
                seen.push(String::from_utf8(line.to_vec()).unwrap());
                true
            })
            .unwrap();
            assert_eq!(seen, vec!["last", "", "second is longer than the block size", "first line"], "chunk {chunk}");
        }
        let mut count = 0;
        for_each_line_rev(&path, 4, |_| {
            count += 1;
            count < 2
        })
        .unwrap();
        assert_eq!(count, 2, "a visit that returns false stops the read");
    }

    #[test]
    fn transcripts_round_trip_and_pages_are_newest_first_across_a_restart() {
        let dir = state("round-trip");
        let files = Files::open(&dir);
        for seq in 1..=5 {
            assert_eq!(files.append_transcript(7, seq, 2, "step", 100 + seq, &json!({ "n": seq })), Ok(true));
        }
        assert_eq!(seqs(&files.transcript(7, 3).unwrap()), vec![3, 4, 5], "the newest three, oldest first");
        assert_eq!(files.transcript_cids().unwrap(), vec![7]);
        drop(files);

        let reopened = Files::open(&dir);
        assert_eq!(seqs(&reopened.transcript(7, 500).unwrap()), vec![1, 2, 3, 4, 5]);
        assert_eq!(reopened.transcript(7, 500).unwrap()[4].4, json!({ "n": 5 }));
        assert_eq!(reopened.append_transcript(7, 6, 2, "step", 106, &json!({})), Ok(true));
        assert_eq!(seqs(&reopened.transcript(7, 2).unwrap()), vec![5, 6]);
    }

    #[test]
    fn a_duplicate_seq_is_ignored_even_after_rotation() {
        let dir = state("duplicate");
        let files = Files::open_with(&dir, 150, CHUNK);
        for seq in 1..=12 {
            assert_eq!(files.append_transcript(9, seq, 1, "step", seq, &json!({ "n": seq })), Ok(true));
        }
        // Duplicates are found in the kept generations: the oldest kept record and the newest.
        let rows = files.transcript(9, 500).unwrap();
        let oldest_kept = rows[0].0;
        assert!(oldest_kept > 1, "the test needs a rotation to have dropped the oldest records");
        assert_eq!(files.append_transcript(9, oldest_kept, 1, "step", 3, &json!({ "again": true })), Ok(false));
        assert_eq!(files.append_transcript(9, 12, 1, "step", 12, &json!({})), Ok(false), "the newest seq too");
        let rows = files.transcript(9, 500).unwrap();
        assert!(rows.iter().all(|r| r.4 != json!({ "again": true })), "the duplicate's data is not stored");
        let mut sorted = seqs(&rows);
        sorted.dedup();
        assert_eq!(sorted.len(), rows.len(), "no seq appears twice");
    }

    #[test]
    fn a_torn_last_line_is_skipped_and_the_next_record_starts_clean() {
        let dir = state("torn");
        let files = Files::open(&dir);
        files.append_transcript(4, 1, 1, "step", 1, &json!({})).unwrap();
        files.append_transcript(4, 2, 1, "step", 2, &json!({})).unwrap();
        drop(files);
        let path = dir.join("transcripts").join("4.jsonl");
        let mut torn = fs::OpenOptions::new().append(true).open(&path).unwrap();
        torn.write_all(br#"{"seq":3,"work":1,"ki"#).unwrap();
        drop(torn);

        let files = Files::open(&dir);
        assert_eq!(seqs(&files.transcript(4, 500).unwrap()), vec![1, 2], "the torn line is not a record");
        assert_eq!(files.append_transcript(4, 3, 1, "step", 3, &json!({ "fresh": true })), Ok(true));
        let rows = files.transcript(4, 500).unwrap();
        assert_eq!(seqs(&rows), vec![1, 2, 3]);
        assert_eq!(rows[2].4, json!({ "fresh": true }), "the new record is intact, not joined to the torn bytes");
    }

    #[test]
    fn rotation_keeps_two_generations_and_pages_span_them() {
        let dir = state("rotation");
        let files = Files::open_with(&dir, 200, CHUNK);
        for seq in 1..=30 {
            files.append_transcript(5, seq, 1, "step", seq, &json!({ "pad": "x".repeat(20) })).unwrap();
        }
        let folder = dir.join("transcripts");
        assert!(folder.join("5.jsonl").exists());
        assert!(folder.join("5.1.jsonl").exists());
        assert!(folder.join("5.2.jsonl").exists());
        assert!(!folder.join("5.3.jsonl").exists(), "only two rotations are kept");
        let rows = seqs(&files.transcript(5, 500).unwrap());
        assert_eq!(*rows.last().unwrap(), 30);
        assert!(rows.windows(2).all(|w| w[1] == w[0] + 1), "the retained records are contiguous: {rows:?}");
        assert!(rows[0] > 1, "the oldest generation was dropped");
        assert_eq!(seqs(&files.transcript(5, 3).unwrap()), vec![28, 29, 30]);
    }

    #[test]
    fn infractions_page_newest_first_and_keep_their_ids_across_a_restart() {
        let dir = state("infractions");
        let infraction = |cid, at_ms, detail: &str| Infraction {
            cid,
            law: "law.test".into(),
            at_ms,
            tool: "tool.test".into(),
            detail: detail.into(),
        };
        let files = Files::open(&dir);
        files.append_infraction(&infraction(1, 10, "a")).unwrap();
        files.append_infraction(&infraction(2, 20, "b")).unwrap();
        files.append_infraction(&infraction(1, 20, "c")).unwrap();
        drop(files);

        let files = Files::open(&dir);
        files.append_infraction(&infraction(1, 30, "d")).unwrap();
        let details = |rows: Vec<Infraction>| rows.into_iter().map(|r| r.detail).collect::<Vec<_>>();
        assert_eq!(details(files.infractions(None, 10).unwrap()), vec!["d", "c", "b", "a"]);
        assert_eq!(details(files.infractions(Some(1), 10).unwrap()), vec!["d", "c", "a"]);
        assert_eq!(details(files.infractions(Some(1), 1).unwrap()), vec!["d"]);
        let ids: Vec<u64> = {
            let text = fs::read_to_string(dir.join("infractions.jsonl")).unwrap();
            text.lines().map(|l| serde_json::from_str::<Value>(l).unwrap()["id"].as_u64().unwrap()).collect()
        };
        assert_eq!(ids, vec![1, 2, 3, 4], "ids continue after the restart");
    }
}
