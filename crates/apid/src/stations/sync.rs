//! The work-folder sync, the hub's half (docs/design/workstations.md section 8).
//!
//! Each workstation's work folder is mirrored by its daemon (apz-workstation's `sync.rs`). The hub keeps its own mirror:
//! one row per path in `station_sync_files` (db/sync.rs), and the content of every version under `sync/<sha256>` in the
//! object store. A pass (`reconcile`) compares the station's manifest with those rows, path by path, and moves what
//! changed on one side to the other. Each row keeps `base`, the content both sides last agreed on:
//!
//! - only the station changed: the hub pulls the file (or records its deletion);
//! - only the hub changed: the hub pushes its copy (or the deletion) against `base`, so the station refuses if it moved;
//! - both changed: a conflict. The hub's copy stays at the path on both sides, and the station's copy is kept as a
//!   conflict copy `<name> (conflict <station> <yyyy-mm-dd hhmm>).<ext>` on both sides. It is audited (`sync_conflict`)
//!   and announced on the bus (`station_sync_conflict`);
//! - deleted on one side, edited on the other: the edit survives.
//!
//! A pass runs when the station reports a change (`Event::SyncChanged`), when its link comes up, and when the owner
//! writes a file here (`kick`). A pass moves at most [`Limits`] and asks for another when it stops short. Owner writes
//! go straight to the rows: a pass saves only the rows no owner write changed under it, and the write asks for a pass.
//!
//! Routes (api.rs): `GET .../sync` (state), `GET .../files?prefix=` (the mirror), `GET|PUT|DELETE .../files/{path}`,
//! and `POST .../files/{path}/deliver` (the file into the Library).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use apz_station::{
    Op, SYNC_CHUNK, SYNC_FILE_CAP, SyncEntry, SyncManifest, SyncOp, SyncPull, SyncPushed,
    valid_path,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::link::StationLink;
use super::ops::{self, Failure};
use crate::deliverables::{Deliverable, Upload, clean_name, guess_type};
use crate::skills::sha256_hex;
use crate::state::App;

/// How long one request to a station may take: a chunk of a file, or its manifest.
pub const TIMEOUT: Duration = Duration::from_secs(60);

/// One path's row: the hub's copy (`hash`, None once deleted), what both sides last agreed (`base`), and where it came
/// from when it is a conflict copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    pub path: String,
    pub hash: Option<String>,
    pub size: u64,
    pub mtime_ms: u64,
    pub version: u64,
    pub base: Option<String>,
    pub conflict_of: Option<String>,
    pub synced_ms: u64,
}

/// What one pass may move before it stops: files, and bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub files: usize,
    pub bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            files: 500,
            bytes: 256 << 20,
        }
    }
}

/// A conflict a pass settled: the station's copy is now `copy`, beside the file at `path`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Conflict {
    pub path: String,
    pub copy: String,
}

/// What a pass did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub pulled: u64,
    pub pushed: u64,
    pub deleted: u64,
    pub bytes: u64,
    pub conflicts: Vec<Conflict>,
    /// Paths that could not be settled this pass, with the reason; they are tried again next time.
    pub errors: Vec<(String, String)>,
    /// Work was left: the limits stopped the pass, or an owner's write raced one of its rows.
    pub more: bool,
}

/// The station's side of a pass.
pub trait Remote {
    fn manifest(&self) -> Result<Vec<SyncEntry>, String>;
    /// The content of `path` at `hash`, verified by the caller.
    fn pull(&self, path: &str, hash: &str) -> Result<Vec<u8>, String>;
    /// Writes `bytes` as `path`; `base` is the version the hub last saw (None for a new file).
    fn push(&self, path: &str, hash: &str, bytes: &[u8], base: Option<&str>) -> Result<(), String>;
    fn delete(&self, path: &str, base: &str) -> Result<(), String>;
}

/// The content store the hub keeps versions in.
pub trait Blobs {
    fn put(&self, bytes: &[u8]) -> Result<String, String>;
    fn get(&self, hash: &str) -> Result<Vec<u8>, String>;
}

/// The rows of each workstation.
pub trait Ledger {
    fn rows(&self, station: &str) -> Result<Vec<Row>, String>;
    fn save(&self, station: &str, rows: &[Row]) -> Result<(), String>;
}

/// Runs one pass for `station`: the reconcile of its manifest with the rows, moving at most `limits`.
pub fn reconcile(
    station: &str,
    station_name: &str,
    remote: &dyn Remote,
    blobs: &dyn Blobs,
    ledger: &dyn Ledger,
    now: u64,
    limits: Limits,
) -> Result<Outcome, String> {
    let theirs: BTreeMap<String, SyncEntry> = remote
        .manifest()?
        .into_iter()
        .filter(|entry| valid_path(&entry.path))
        .map(|entry| (entry.path.clone(), entry))
        .collect();
    let snapshot: BTreeMap<String, Row> = ledger
        .rows(station)?
        .into_iter()
        .map(|row| (row.path.clone(), row))
        .collect();
    let mut names: BTreeSet<String> = theirs.keys().cloned().collect();
    names.extend(snapshot.keys().cloned());
    let mut pass = Pass {
        station_name,
        remote,
        blobs,
        now,
        rows: snapshot.clone(),
        changed: BTreeMap::new(),
        names,
        transfers: 0,
        out: Outcome::default(),
    };
    let paths: Vec<String> = pass.names.iter().cloned().collect();
    for path in &paths {
        let station_side = theirs.get(path);
        if pass.agreed(path, station_side) {
            continue;
        }
        if pass.transfers >= limits.files || pass.out.bytes >= limits.bytes {
            pass.out.more = true;
            break;
        }
        pass.transfers += 1;
        if let Err(reason) = pass.settle(path, station_side) {
            pass.out.errors.push((path.clone(), reason));
        }
    }
    pass.save(station, ledger, &snapshot)?;
    Ok(pass.out)
}

struct Pass<'a> {
    station_name: &'a str,
    remote: &'a dyn Remote,
    blobs: &'a dyn Blobs,
    now: u64,
    /// The rows as the pass leaves them.
    rows: BTreeMap<String, Row>,
    /// The rows the pass changed, to save.
    changed: BTreeMap<String, Row>,
    /// Every path the station or the hub has, conflict copies included: a copy's name must be free.
    names: BTreeSet<String>,
    transfers: usize,
    out: Outcome,
}

impl Pass<'_> {
    fn row(&self, path: &str) -> Option<Row> {
        self.rows.get(path).cloned()
    }

    fn put_row(&mut self, row: Row) {
        self.changed.insert(row.path.clone(), row.clone());
        self.rows.insert(row.path.clone(), row);
    }

    /// Whether hub and station hold the same content for `path`. Then nothing moves, but the base is brought up to
    /// date when it was not.
    fn agreed(&mut self, path: &str, station_side: Option<&SyncEntry>) -> bool {
        let theirs = station_side.map(|entry| entry.hash.clone());
        let row = self.row(path);
        if row.as_ref().and_then(|r| r.hash.clone()) != theirs {
            return false;
        }
        if let Some(mut row) = row
            && row.base != theirs
        {
            row.base = theirs;
            row.synced_ms = self.now;
            self.put_row(row);
        }
        true
    }

    fn settle(&mut self, path: &str, station_side: Option<&SyncEntry>) -> Result<(), String> {
        let row = self.row(path);
        let hub = row.as_ref().and_then(|r| r.hash.clone());
        let base = row.as_ref().and_then(|r| r.base.clone());
        let hub_changed = hub != base;
        let station_changed = station_side.map(|entry| entry.hash.clone()) != base;
        match (hub_changed, station_changed) {
            (false, true) => match station_side {
                Some(entry) => self.adopt(path, entry, row, None),
                None => self.forget_on_hub(row),
            },
            (true, false) => self.send(path, hub, base, row),
            (true, true) => match (hub, station_side) {
                // The station deleted what the hub edited: the edit goes back to the station as a new file.
                (Some(hash), None) => self.send(path, Some(hash), None, row),
                // The hub deleted what the station edited: the station's edit stays.
                (None, Some(entry)) => self.adopt(path, entry, row, None),
                (Some(hash), Some(entry)) => self.conflict(path, &hash, entry, row),
                (None, None) => Ok(()),
            },
            (false, false) => Ok(()),
        }
    }

    /// The station's version comes to the hub (its pull, verified, stored).
    fn adopt(
        &mut self,
        path: &str,
        entry: &SyncEntry,
        row: Option<Row>,
        conflict_of: Option<String>,
    ) -> Result<(), String> {
        let bytes = self.fetch(path, &entry.hash)?;
        self.out.pulled += 1;
        self.out.bytes += bytes.len() as u64;
        let (version, kept_conflict) = match &row {
            Some(r) => (r.version + 1, r.conflict_of.clone()),
            None => (1, None),
        };
        self.put_row(Row {
            path: path.to_string(),
            hash: Some(entry.hash.clone()),
            size: bytes.len() as u64,
            mtime_ms: self.now,
            version,
            base: Some(entry.hash.clone()),
            conflict_of: conflict_of.or(kept_conflict),
            synced_ms: self.now,
        });
        Ok(())
    }

    /// The station deleted a file the hub had not changed: the hub's row records the deletion.
    fn forget_on_hub(&mut self, row: Option<Row>) -> Result<(), String> {
        let Some(mut row) = row else { return Ok(()) };
        row.hash = None;
        row.base = None;
        row.version += 1;
        row.synced_ms = self.now;
        self.out.deleted += 1;
        self.put_row(row);
        Ok(())
    }

    /// The hub's version (`hub` Some) or the hub's deletion (`hub` None) goes to the station, against `base`.
    fn send(
        &mut self,
        path: &str,
        hub: Option<String>,
        base: Option<String>,
        row: Option<Row>,
    ) -> Result<(), String> {
        let now = self.now;
        match hub {
            Some(hash) => {
                let bytes = self.blobs.get(&hash)?;
                self.remote.push(path, &hash, &bytes, base.as_deref())?;
                self.out.pushed += 1;
                self.out.bytes += bytes.len() as u64;
                let mut row = row.ok_or("the hub has no row for the file it sends")?;
                row.base = Some(hash);
                row.synced_ms = now;
                self.put_row(row);
            }
            None => {
                if let Some(old) = &base {
                    self.remote.delete(path, old)?;
                }
                self.out.deleted += 1;
                let mut row = row.ok_or("the hub has no row for the file it deletes")?;
                row.base = None;
                row.synced_ms = now;
                self.put_row(row);
            }
        }
        Ok(())
    }

    /// Both sides changed `path`. The station's copy is saved first, under a conflict name on both sides, so nothing is
    /// lost if a later step fails; then the hub's copy takes the path on the station.
    fn conflict(
        &mut self,
        path: &str,
        hub: &str,
        entry: &SyncEntry,
        row: Option<Row>,
    ) -> Result<(), String> {
        let copy = self.copy_name(path);
        let bytes = self.fetch(path, &entry.hash)?;
        self.remote.push(&copy, &entry.hash, &bytes, None)?;
        self.names.insert(copy.clone());
        self.out.bytes += bytes.len() as u64;
        self.put_row(Row {
            path: copy.clone(),
            hash: Some(entry.hash.clone()),
            size: bytes.len() as u64,
            mtime_ms: self.now,
            version: 1,
            base: Some(entry.hash.clone()),
            conflict_of: Some(path.to_string()),
            synced_ms: self.now,
        });
        let mine = self.blobs.get(hub)?;
        self.remote.push(path, hub, &mine, Some(&entry.hash))?;
        self.out.pushed += 1;
        self.out.bytes += mine.len() as u64;
        let mut row = row.ok_or("the hub has no row for the conflicting file")?;
        row.base = Some(hub.to_string());
        row.synced_ms = self.now;
        self.put_row(row);
        self.out.conflicts.push(Conflict {
            path: path.to_string(),
            copy,
        });
        Ok(())
    }

    /// The station's content, pulled and checked against its hash, and stored in the hub's content store.
    fn fetch(&self, path: &str, hash: &str) -> Result<Vec<u8>, String> {
        let bytes = self.remote.pull(path, hash)?;
        if sha256_hex(&bytes) != hash {
            return Err(format!("the station's {path} does not hash to {hash}"));
        }
        self.blobs.put(&bytes)?;
        Ok(bytes)
    }

    fn copy_name(&self, path: &str) -> String {
        let mut n = 1;
        loop {
            let name = conflict_name(path, self.station_name, self.now, n);
            if !self.names.contains(&name) {
                return name;
            }
            n += 1;
        }
    }

    /// Saves the changed rows, except those an owner's write changed meanwhile (they wait for the next pass).
    fn save(
        &mut self,
        station: &str,
        ledger: &dyn Ledger,
        snapshot: &BTreeMap<String, Row>,
    ) -> Result<(), String> {
        if self.changed.is_empty() {
            return Ok(());
        }
        let current: BTreeMap<String, Row> = ledger
            .rows(station)?
            .into_iter()
            .map(|row| (row.path.clone(), row))
            .collect();
        let mut keep = Vec::new();
        for (path, row) in std::mem::take(&mut self.changed) {
            if current.get(&path) == snapshot.get(&path) {
                keep.push(row);
            } else {
                self.out.more = true;
            }
        }
        ledger.save(station, &keep)
    }
}

/// The name a conflict copy takes: `<stem> (conflict <station> <yyyy-mm-dd hhmm>).<ext>` in the file's folder, UTC.
/// `n` above 1 adds the count, for a second copy in the same minute.
pub fn conflict_name(path: &str, station: &str, now_ms: u64, n: u32) -> String {
    let (dir, file) = match path.rsplit_once('/') {
        Some((dir, file)) => (Some(dir), file),
        None => (None, path),
    };
    let (stem, ext) = match file.rfind('.') {
        Some(at) if at > 0 => (&file[..at], &file[at..]),
        _ => (file, ""),
    };
    let station: String = station
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let station = if station.is_empty() {
        "station".to_string()
    } else {
        station
    };
    let count = if n > 1 {
        format!(" {n}")
    } else {
        String::new()
    };
    let name = format!(
        "{stem} (conflict {station} {}{count}){ext}",
        utc_stamp(now_ms)
    );
    match dir {
        Some(dir) => format!("{dir}/{name}"),
        None => name,
    }
}

/// `yyyy-mm-dd hhmm` in UTC for a time in milliseconds since the Unix epoch.
fn utc_stamp(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let days = secs.div_euclid(86_400);
    let of_day = secs.rem_euclid(86_400);
    let (year, month, day) = civil(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}{:02}",
        of_day / 3600,
        (of_day % 3600) / 60
    )
}

/// The civil date of a day count since 1970-01-01 (proleptic Gregorian; H. Hinnant's algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The station's requests over its live link.
pub struct LinkRemote<'a>(pub &'a StationLink);

fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value)
        .map_err(|e| format!("the station's answer is not as the protocol says: {e}"))
}

impl Remote for LinkRemote<'_> {
    fn manifest(&self) -> Result<Vec<SyncEntry>, String> {
        let manifest: SyncManifest = parse(self.0.request(Op::Sync(SyncOp::Manifest), TIMEOUT)?)?;
        Ok(manifest.entries)
    }

    fn pull(&self, path: &str, hash: &str) -> Result<Vec<u8>, String> {
        let mut out: Vec<u8> = Vec::new();
        loop {
            let offset = out.len() as u64;
            let op = SyncOp::Pull {
                path: path.to_string(),
                hash: hash.to_string(),
                offset: Some(offset),
            };
            let chunk: SyncPull = parse(self.0.request(Op::Sync(op), TIMEOUT)?)?;
            let bytes = STANDARD
                .decode(&chunk.data_b64)
                .map_err(|e| format!("a chunk of {path} is not base64: {e}"))?;
            if chunk.offset != offset || chunk.size > SYNC_FILE_CAP || bytes.len() > SYNC_CHUNK {
                return Err(format!("a chunk of {path} is out of the protocol's bounds"));
            }
            if bytes.is_empty() && !chunk.eof {
                return Err(format!("the station sent no bytes of {path} and no end"));
            }
            out.extend_from_slice(&bytes);
            if chunk.eof {
                return if out.len() as u64 == chunk.size {
                    Ok(out)
                } else {
                    Err(format!(
                        "{path} ended at {} bytes, not {}",
                        out.len(),
                        chunk.size
                    ))
                };
            }
        }
    }

    fn push(&self, path: &str, hash: &str, bytes: &[u8], base: Option<&str>) -> Result<(), String> {
        let total = bytes.len() as u64;
        if total > SYNC_FILE_CAP {
            return Err(format!(
                "{path} is {total} bytes; the sync takes files up to {SYNC_FILE_CAP}"
            ));
        }
        let mut sent = 0usize;
        loop {
            let end = (sent + SYNC_CHUNK).min(bytes.len());
            let op = SyncOp::Push {
                path: path.to_string(),
                hash: hash.to_string(),
                data_b64: STANDARD.encode(&bytes[sent..end]),
                base_hash: base.map(str::to_string),
                offset: Some(sent as u64),
                total: Some(total),
            };
            let reply: SyncPushed = parse(self.0.request(Op::Sync(op), TIMEOUT)?)?;
            sent = end;
            if sent as u64 == total {
                return if reply.hash.as_deref() == Some(hash) {
                    Ok(())
                } else {
                    Err(format!("the station did not confirm {path} at {hash}"))
                };
            }
        }
    }

    fn delete(&self, path: &str, base: &str) -> Result<(), String> {
        let op = SyncOp::Delete {
            path: path.to_string(),
            base_hash: base.to_string(),
        };
        self.0.request(Op::Sync(op), TIMEOUT).map(|_| ())
    }
}

/// The hub's content store: the Library's object store (Garage, or its folder), under `sync/<sha256>`.
pub struct Objects<'a>(pub &'a crate::deliverables::Store);

fn object_key(hash: &str) -> Result<String, String> {
    let plain = hash.len() == 64
        && hash
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c));
    if !plain {
        return Err(format!("{hash} is not a sha256"));
    }
    Ok(format!("sync/{hash}"))
}

impl Blobs for Objects<'_> {
    fn put(&self, bytes: &[u8]) -> Result<String, String> {
        let hash = sha256_hex(bytes);
        self.0.put_object(&object_key(&hash)?, bytes)?;
        Ok(hash)
    }

    fn get(&self, hash: &str) -> Result<Vec<u8>, String> {
        let bytes = self.0.get_object(&object_key(hash)?)?;
        if sha256_hex(&bytes) != hash {
            return Err(format!("the stored content does not hash to {hash}"));
        }
        Ok(bytes)
    }
}

/// The hub's database rows (`station_sync_files`, or the state folder without one).
pub struct DbLedger<'a>(pub &'a crate::db::Db);

impl Ledger for DbLedger<'_> {
    fn rows(&self, station: &str) -> Result<Vec<Row>, String> {
        self.0.station_sync_rows(station)
    }

    fn save(&self, station: &str, rows: &[Row]) -> Result<(), String> {
        if rows.is_empty() {
            Ok(())
        } else {
            self.0.save_station_sync_rows(station, rows)
        }
    }
}

/// The passes in progress, by station: a station with a pass running has a flag saying another is wanted.
#[derive(Default)]
pub struct Gate {
    passes: Mutex<BTreeMap<String, bool>>,
}

impl Gate {
    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, bool>> {
        self.passes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Claims a pass for `station`. False when one runs already: it is asked to run once more.
    fn begin(&self, station: &str) -> bool {
        let mut passes = self.lock();
        if let Some(again) = passes.get_mut(station) {
            *again = true;
            return false;
        }
        passes.insert(station.to_string(), false);
        true
    }

    /// Ends a pass. True when another should run: the pass left work, or a change came in while it ran.
    fn end(&self, station: &str, more: bool) -> bool {
        let mut passes = self.lock();
        let again = more || passes.get(station).copied().unwrap_or(false);
        if again {
            passes.insert(station.to_string(), false);
        } else {
            passes.remove(station);
        }
        again
    }
}

/// Asks for a pass for `station`, on its own thread. A pass already running takes one more round instead.
pub fn kick(app: &App, station: &str) {
    if !app.stations.sync.begin(station) {
        return;
    }
    let weak = app.handle();
    let station = station.to_string();
    std::thread::spawn(move || {
        loop {
            let Some(app) = weak.upgrade() else { return };
            let more = match run(&app, &station) {
                Ok(outcome) => outcome.more,
                Err(reason) => {
                    eprintln!("apid: workstation {station}: work-folder sync failed: {reason}");
                    false
                }
            };
            if !app.stations.sync.end(&station, more) {
                return;
            }
        }
    });
}

/// One pass over the station's live link. A station that is not linked has nothing to do: its link starts a pass.
fn run(app: &App, station: &str) -> Result<Outcome, String> {
    let (Some(link), Some(record)) = (app.stations.live(station), app.stations.station(station))
    else {
        return Ok(Outcome::default());
    };
    let outcome = reconcile(
        station,
        &record.name,
        &LinkRemote(&link),
        &Objects(&app.deliverables),
        &DbLedger(&app.db),
        apz_plaza::now_ms(),
        Limits::default(),
    )?;
    for conflict in &outcome.conflicts {
        let _ = ops::audit(
            app,
            Some(station),
            None,
            "sync_conflict",
            "station",
            json!({"path": conflict.path, "copy": conflict.copy}),
        );
        app.bus.publish(
            "station_sync_conflict",
            &json!({"station": station, "path": conflict.path, "copy": conflict.copy}),
        );
    }
    for (path, reason) in &outcome.errors {
        eprintln!("apid: workstation {station}: sync of {path} failed: {reason}");
    }
    if outcome.pulled + outcome.pushed + outcome.deleted > 0 {
        app.bus.publish(
            "station_sync",
            &json!({"station": station, "pulled": outcome.pulled, "pushed": outcome.pushed, "deleted": outcome.deleted}),
        );
    }
    Ok(outcome)
}

/// A file of the mirror as the routes show it.
#[derive(Debug, Serialize)]
pub struct FileView {
    pub path: String,
    pub hash: Option<String>,
    pub size: u64,
    pub mtime_ms: u64,
    pub version: u64,
    pub conflict_of: Option<String>,
    pub deleted: bool,
}

fn view(row: &Row) -> FileView {
    FileView {
        path: row.path.clone(),
        hash: row.hash.clone(),
        size: row.size,
        mtime_ms: row.mtime_ms,
        version: row.version,
        conflict_of: row.conflict_of.clone(),
        deleted: row.hash.is_none(),
    }
}

fn server_error(message: impl ToString) -> Failure {
    (500, message.to_string())
}

fn station_or_404(app: &App, station: &str) -> Result<(), Failure> {
    app.stations
        .station(station)
        .map(|_| ())
        .ok_or((404, "no such workstation".to_string()))
}

fn rows_of(app: &App, station: &str) -> Result<Vec<Row>, Failure> {
    app.db.station_sync_rows(station).map_err(server_error)
}

/// The sync's state for the Workstations page: the files, their bytes, the last sync, and the conflict copies.
pub fn state(app: &App, station: &str) -> Result<Value, Failure> {
    station_or_404(app, station)?;
    let rows = rows_of(app, station)?;
    let live: Vec<&Row> = rows.iter().filter(|r| r.hash.is_some()).collect();
    let conflicts: Vec<FileView> = live
        .iter()
        .filter(|r| r.conflict_of.is_some())
        .map(|r| view(r))
        .collect();
    Ok(json!({
        "station": station,
        "linked": app.stations.live(station).is_some(),
        "files": live.len(),
        "bytes": live.iter().map(|r| r.size).sum::<u64>(),
        "last_sync_ms": rows.iter().map(|r| r.synced_ms).max().unwrap_or(0),
        "conflicts": conflicts,
    }))
}

/// The mirror's live files under `prefix`, by path.
pub fn list(app: &App, station: &str, prefix: &str) -> Result<Vec<FileView>, Failure> {
    station_or_404(app, station)?;
    Ok(rows_of(app, station)?
        .iter()
        .filter(|r| r.hash.is_some() && r.path.starts_with(prefix))
        .map(view)
        .collect())
}

/// A live file's content and its row.
pub fn read(app: &App, station: &str, path: &str) -> Result<(Vec<u8>, FileView), Failure> {
    station_or_404(app, station)?;
    let row = live_row(app, station, path)?;
    let bytes = Objects(&app.deliverables)
        .get(row.hash.as_deref().unwrap_or_default())
        .map_err(server_error)?;
    Ok((bytes, view(&row)))
}

fn live_row(app: &App, station: &str, path: &str) -> Result<Row, Failure> {
    rows_of(app, station)?
        .into_iter()
        .find(|r| r.path == path && r.hash.is_some())
        .ok_or((404, format!("no file {path} in the work folder")))
}

/// The owner's write of a file: stored, recorded as a hub change, and pushed to the station by the next pass.
pub fn put(
    app: &App,
    station: &str,
    path: &str,
    bytes: &[u8],
    actor: &str,
) -> Result<FileView, Failure> {
    station_or_404(app, station)?;
    if !valid_path(path) {
        return Err((400, format!("{path:?} is not a path the sync takes")));
    }
    if bytes.len() as u64 > SYNC_FILE_CAP {
        return Err((
            413,
            format!("the sync takes files up to {SYNC_FILE_CAP} bytes"),
        ));
    }
    let hash = Objects(&app.deliverables)
        .put(bytes)
        .map_err(server_error)?;
    let previous = rows_of(app, station)?.into_iter().find(|r| r.path == path);
    let row = Row {
        path: path.to_string(),
        hash: Some(hash.clone()),
        size: bytes.len() as u64,
        mtime_ms: apz_plaza::now_ms(),
        version: previous.as_ref().map_or(0, |r| r.version) + 1,
        base: previous.as_ref().and_then(|r| r.base.clone()),
        conflict_of: previous.as_ref().and_then(|r| r.conflict_of.clone()),
        synced_ms: previous.as_ref().map_or(0, |r| r.synced_ms),
    };
    app.db
        .save_station_sync_rows(station, std::slice::from_ref(&row))
        .map_err(server_error)?;
    ops::audit(
        app,
        Some(station),
        None,
        "sync_put",
        actor,
        json!({"path": path, "hash": hash, "size": bytes.len()}),
    )
    .map_err(server_error)?;
    kick(app, station);
    Ok(view(&row))
}

/// The owner's deletion of a file: the row records it, and the next pass removes it from the station.
pub fn remove(app: &App, station: &str, path: &str, actor: &str) -> Result<FileView, Failure> {
    station_or_404(app, station)?;
    let mut row = live_row(app, station, path)?;
    row.hash = None;
    row.version += 1;
    row.mtime_ms = apz_plaza::now_ms();
    app.db
        .save_station_sync_rows(station, std::slice::from_ref(&row))
        .map_err(server_error)?;
    ops::audit(
        app,
        Some(station),
        None,
        "sync_delete",
        actor,
        json!({"path": path}),
    )
    .map_err(server_error)?;
    kick(app, station);
    Ok(view(&row))
}

/// Copies a mirrored file into the Library as a deliverable (the same entry an agent's deliver makes).
pub fn deliver(
    app: &App,
    station: &str,
    path: &str,
    title: Option<String>,
    description: Option<String>,
    task: Option<String>,
    actor: &str,
) -> Result<Deliverable, Failure> {
    station_or_404(app, station)?;
    let row = live_row(app, station, path)?;
    let bytes = Objects(&app.deliverables)
        .get(row.hash.as_deref().unwrap_or_default())
        .map_err(server_error)?;
    let base = path.rsplit('/').next().unwrap_or(path);
    let name = clean_name(base).map_err(|e| (400, e))?;
    let upload = Upload {
        title: title.unwrap_or_else(|| name.clone()),
        content_type: guess_type(&name).to_string(),
        name,
        description,
        task,
        author_cid: 0,
        author: actor.to_string(),
        kind: "deliverable".to_string(),
        audience: "public".to_string(),
        allowed: Vec::new(),
    };
    let item = app.deliverables.add(upload, &bytes).map_err(server_error)?;
    app.bus.publish("file", &item);
    ops::audit(
        app,
        Some(station),
        None,
        "sync_deliver",
        actor,
        json!({"path": path, "file": item.id}),
    )
    .map_err(server_error)?;
    Ok(item)
}

#[cfg(test)]
mod tests {
    use super::*;
    use apz_station::{HubFrame, StationFrame};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    const NOW: u64 = 1_791_991_920_000; // 2026-10-14 15:32 UTC

    fn sha(bytes: &[u8]) -> String {
        sha256_hex(bytes)
    }

    /// The station in memory: its files by path, and the calls the hub made.
    #[derive(Default)]
    struct Station {
        files: RefCell<BTreeMap<String, Vec<u8>>>,
        pulls: RefCell<usize>,
        fail_pull: RefCell<Option<String>>,
    }

    impl Station {
        fn with(files: &[(&str, &[u8])]) -> Self {
            let station = Station::default();
            for (path, bytes) in files {
                station
                    .files
                    .borrow_mut()
                    .insert(path.to_string(), bytes.to_vec());
            }
            station
        }

        fn edit(&self, path: &str, bytes: &[u8]) {
            self.files
                .borrow_mut()
                .insert(path.to_string(), bytes.to_vec());
        }
    }

    impl Remote for Station {
        fn manifest(&self) -> Result<Vec<SyncEntry>, String> {
            Ok(self
                .files
                .borrow()
                .iter()
                .map(|(p, b)| SyncEntry {
                    path: p.clone(),
                    hash: sha(b),
                    size: b.len() as u64,
                })
                .collect())
        }

        fn pull(&self, path: &str, hash: &str) -> Result<Vec<u8>, String> {
            *self.pulls.borrow_mut() += 1;
            if let Some(reason) = self.fail_pull.borrow().clone() {
                return Err(reason);
            }
            let files = self.files.borrow();
            let bytes = files.get(path).ok_or("no such file")?;
            if sha(bytes) != hash {
                return Err("changed".into());
            }
            Ok(bytes.clone())
        }

        fn push(
            &self,
            path: &str,
            hash: &str,
            bytes: &[u8],
            base: Option<&str>,
        ) -> Result<(), String> {
            let mut files = self.files.borrow_mut();
            let current = files.get(path).map(|b| sha(b));
            if current.as_deref() != base {
                return Err(format!("conflict: {path} changed on the station"));
            }
            if sha(bytes) != hash {
                return Err("hash mismatch".into());
            }
            files.insert(path.to_string(), bytes.to_vec());
            Ok(())
        }

        fn delete(&self, path: &str, base: &str) -> Result<(), String> {
            let mut files = self.files.borrow_mut();
            match files.get(path) {
                Some(b) if sha(b) == base => {
                    files.remove(path);
                    Ok(())
                }
                Some(_) => Err("conflict".into()),
                None => Ok(()),
            }
        }
    }

    #[derive(Default)]
    struct Hub {
        blobs: RefCell<HashMap<String, Vec<u8>>>,
        rows: RefCell<BTreeMap<String, Row>>,
    }

    impl Blobs for Hub {
        fn put(&self, bytes: &[u8]) -> Result<String, String> {
            let hash = sha(bytes);
            self.blobs.borrow_mut().insert(hash.clone(), bytes.to_vec());
            Ok(hash)
        }

        fn get(&self, hash: &str) -> Result<Vec<u8>, String> {
            self.blobs
                .borrow()
                .get(hash)
                .cloned()
                .ok_or_else(|| format!("no blob {hash}"))
        }
    }

    impl Ledger for Hub {
        fn rows(&self, _station: &str) -> Result<Vec<Row>, String> {
            Ok(self.rows.borrow().values().cloned().collect())
        }

        fn save(&self, _station: &str, rows: &[Row]) -> Result<(), String> {
            for row in rows {
                self.rows.borrow_mut().insert(row.path.clone(), row.clone());
            }
            Ok(())
        }
    }

    /// Hub-side write, as the owner's PUT does it (a row with the hub's content).
    fn hub_write(hub: &Hub, path: &str, bytes: &[u8]) {
        let hash = hub.put(bytes).unwrap();
        let mut rows = hub.rows.borrow_mut();
        let prev = rows.get(path).cloned();
        rows.insert(
            path.to_string(),
            Row {
                path: path.to_string(),
                hash: Some(hash),
                size: bytes.len() as u64,
                mtime_ms: NOW,
                version: prev.as_ref().map_or(0, |r| r.version) + 1,
                base: prev.as_ref().and_then(|r| r.base.clone()),
                conflict_of: None,
                synced_ms: 0,
            },
        );
    }

    fn pass(station: &Station, hub: &Hub) -> Outcome {
        reconcile("ws-7", "desk-1", station, hub, hub, NOW, Limits::default()).unwrap()
    }

    #[test]
    fn a_file_the_station_made_is_pulled_once_and_then_agreed() {
        let station = Station::with(&[("report.csv", b"a,b\n1,2\n")]);
        let hub = Hub::default();
        let first = pass(&station, &hub);
        assert_eq!((first.pulled, first.pushed, first.deleted), (1, 0, 0));
        let row = hub.rows.borrow()["report.csv"].clone();
        assert_eq!(
            (row.hash.as_deref(), row.base.as_deref(), row.version),
            (
                Some(sha(b"a,b\n1,2\n")).as_deref(),
                Some(sha(b"a,b\n1,2\n")).as_deref(),
                1
            )
        );
        assert_eq!(hub.get(&sha(b"a,b\n1,2\n")).unwrap(), b"a,b\n1,2\n");

        let second = pass(&station, &hub);
        assert_eq!(
            second,
            Outcome::default(),
            "a second pass finds nothing to move"
        );
        assert_eq!(*station.pulls.borrow(), 1);
    }

    #[test]
    fn an_owner_upload_is_pushed_with_the_base_the_hub_last_saw() {
        let station = Station::with(&[("notes.md", b"v1")]);
        let hub = Hub::default();
        pass(&station, &hub);
        hub_write(&hub, "notes.md", b"v2 from the owner");
        let out = pass(&station, &hub);
        assert_eq!(out.pushed, 1);
        assert_eq!(station.files.borrow()["notes.md"], b"v2 from the owner");
        assert_eq!(
            hub.rows.borrow()["notes.md"].base,
            Some(sha(b"v2 from the owner"))
        );
    }

    #[test]
    fn a_new_hub_file_goes_to_the_station_and_a_hub_deletion_removes_it() {
        let station = Station::with(&[]);
        let hub = Hub::default();
        hub_write(&hub, "in/brief.txt", b"brief");
        assert_eq!(pass(&station, &hub).pushed, 1);
        assert_eq!(station.files.borrow()["in/brief.txt"], b"brief");

        let mut row = hub.rows.borrow()["in/brief.txt"].clone();
        row.hash = None;
        row.version += 1;
        hub.rows.borrow_mut().insert(row.path.clone(), row);
        let out = pass(&station, &hub);
        assert_eq!(out.deleted, 1);
        assert!(!station.files.borrow().contains_key("in/brief.txt"));
        assert_eq!(hub.rows.borrow()["in/brief.txt"].base, None);
    }

    #[test]
    fn a_station_deletion_is_recorded_on_the_hub_and_the_edit_survives_it() {
        let station = Station::with(&[("gone.txt", b"bye"), ("kept.txt", b"stay")]);
        let hub = Hub::default();
        pass(&station, &hub);
        station.files.borrow_mut().remove("gone.txt");
        station.edit("kept.txt", b"edited on the station");
        let out = pass(&station, &hub);
        assert_eq!(out.deleted, 1);
        assert_eq!(hub.rows.borrow()["gone.txt"].hash, None);
        assert_eq!(
            hub.get(&sha(b"edited on the station")).unwrap(),
            b"edited on the station"
        );

        // Deleted on the hub while the station edited it: the station's edit comes back to the hub.
        let mut row = hub.rows.borrow()["kept.txt"].clone();
        row.hash = None;
        hub.rows.borrow_mut().insert(row.path.clone(), row);
        station.edit("kept.txt", b"edited again");
        let out = pass(&station, &hub);
        assert_eq!(out.pulled, 1);
        assert_eq!(
            hub.rows.borrow()["kept.txt"].hash,
            Some(sha(b"edited again"))
        );
    }

    #[test]
    fn a_change_on_both_sides_keeps_both_copies_and_reports_the_conflict() {
        let station = Station::with(&[("plan.md", b"base text")]);
        let hub = Hub::default();
        pass(&station, &hub);
        station.edit("plan.md", b"the station's text");
        hub_write(&hub, "plan.md", b"the hub's text");

        let out = pass(&station, &hub);
        let copy = "plan (conflict desk-1 2026-10-14 1532).md";
        assert_eq!(
            out.conflicts,
            vec![Conflict {
                path: "plan.md".into(),
                copy: copy.into()
            }]
        );
        assert_eq!(
            station.files.borrow()["plan.md"],
            b"the hub's text",
            "the hub's copy stays at the path"
        );
        assert_eq!(
            station.files.borrow()[copy],
            b"the station's text",
            "the station's copy is kept beside it"
        );
        let rows = hub.rows.borrow();
        assert_eq!(rows[copy].conflict_of.as_deref(), Some("plan.md"));
        assert_eq!(
            rows[copy].hash.as_deref(),
            Some(sha(b"the station's text")).as_deref()
        );
        assert_eq!(rows["plan.md"].base, Some(sha(b"the hub's text")));
        drop(rows);

        let again = pass(&station, &hub);
        assert!(
            again.conflicts.is_empty(),
            "the next pass has nothing to settle"
        );
    }

    #[test]
    fn a_second_conflict_in_the_same_minute_takes_a_numbered_name() {
        assert_eq!(
            conflict_name("a/b.txt", "desk-1", NOW, 1),
            "a/b (conflict desk-1 2026-10-14 1532).txt"
        );
        assert_eq!(
            conflict_name("a/b.txt", "desk-1", NOW, 2),
            "a/b (conflict desk-1 2026-10-14 1532 2).txt"
        );
        assert_eq!(
            conflict_name("README", "", NOW, 1),
            "README (conflict station 2026-10-14 1532)"
        );
        assert_eq!(
            conflict_name(".env", "a b/c", NOW, 1),
            ".env (conflict a-b-c 2026-10-14 1532)"
        );
    }

    #[test]
    fn the_utc_stamp_is_the_calendar_date_and_the_time() {
        assert_eq!(utc_stamp(0), "1970-01-01 0000");
        assert_eq!(utc_stamp(951_782_400_000), "2000-02-29 0000", "a leap day");
        assert_eq!(utc_stamp(NOW), "2026-10-14 1532");
    }

    #[test]
    fn a_failed_pull_is_reported_for_its_path_and_the_rest_still_moves() {
        let station = Station::with(&[("a.txt", b"aaa"), ("b.txt", b"bbb")]);
        *station.fail_pull.borrow_mut() = Some("the link dropped".into());
        let hub = Hub::default();
        let out = pass(&station, &hub);
        assert_eq!(out.errors.len(), 2);
        assert!(
            hub.rows.borrow().is_empty(),
            "nothing is recorded for a file that did not arrive"
        );
        *station.fail_pull.borrow_mut() = None;
        assert_eq!(pass(&station, &hub).pulled, 2);
    }

    #[test]
    fn the_limits_stop_a_pass_and_ask_for_another() {
        let station = Station::with(&[("a.txt", b"1"), ("b.txt", b"2"), ("c.txt", b"3")]);
        let hub = Hub::default();
        let limits = Limits {
            files: 2,
            bytes: u64::MAX,
        };
        let first = reconcile("ws-7", "desk-1", &station, &hub, &hub, NOW, limits).unwrap();
        assert_eq!(first.pulled, 2);
        assert!(first.more);
        let second = reconcile("ws-7", "desk-1", &station, &hub, &hub, NOW, limits).unwrap();
        assert_eq!(second.pulled, 1);
        assert!(!second.more);
    }

    #[test]
    fn a_row_an_owner_wrote_during_a_pass_is_not_overwritten_by_it() {
        struct Racing<'a> {
            hub: &'a Hub,
            reads: RefCell<usize>,
        }
        impl Ledger for Racing<'_> {
            fn rows(&self, station: &str) -> Result<Vec<Row>, String> {
                // The pass reads the rows once at its start and once more before it saves. The owner writes the file
                // in between.
                *self.reads.borrow_mut() += 1;
                if *self.reads.borrow() == 2 {
                    hub_write(self.hub, "a.txt", b"owner's newer text");
                }
                self.hub.rows(station)
            }
            fn save(&self, station: &str, rows: &[Row]) -> Result<(), String> {
                self.hub.save(station, rows)
            }
        }
        let station = Station::with(&[("a.txt", b"v1")]);
        let hub = Hub::default();
        pass(&station, &hub);
        station.edit("a.txt", b"station v2");
        let racing = Racing {
            hub: &hub,
            reads: RefCell::new(0),
        };
        let out = reconcile(
            "ws-7",
            "desk-1",
            &station,
            &hub,
            &racing,
            NOW,
            Limits::default(),
        )
        .unwrap();
        assert!(out.more, "the raced row waits for the next pass");
        assert_eq!(
            hub.rows.borrow()["a.txt"].hash,
            Some(sha(b"owner's newer text"))
        );
    }

    #[test]
    fn a_station_that_answers_the_wrong_hash_is_not_trusted() {
        struct Liar;
        impl Remote for Liar {
            fn manifest(&self) -> Result<Vec<SyncEntry>, String> {
                Ok(vec![SyncEntry {
                    path: "x.txt".into(),
                    hash: sha(b"claimed"),
                    size: 7,
                }])
            }
            fn pull(&self, _: &str, _: &str) -> Result<Vec<u8>, String> {
                Ok(b"other bytes".to_vec())
            }
            fn push(&self, _: &str, _: &str, _: &[u8], _: Option<&str>) -> Result<(), String> {
                Ok(())
            }
            fn delete(&self, _: &str, _: &str) -> Result<(), String> {
                Ok(())
            }
        }
        let hub = Hub::default();
        let out = reconcile("ws-7", "desk-1", &Liar, &hub, &hub, NOW, Limits::default()).unwrap();
        assert_eq!(out.errors.len(), 1);
        assert!(hub.rows.borrow().is_empty());
    }

    #[test]
    fn the_gate_gives_one_pass_per_station_and_one_more_round_for_a_change_during_it() {
        let gate = Gate::default();
        assert!(gate.begin("ws-7"));
        assert!(!gate.begin("ws-7"), "a pass runs: the change is queued");
        assert!(gate.end("ws-7", false), "the queued change gets its round");
        assert!(!gate.end("ws-7", false));
        assert!(
            gate.begin("ws-7"),
            "after the last pass the station is free again"
        );
        assert!(
            gate.end("ws-7", true),
            "a pass that left work asks for another"
        );
        assert!(!gate.end("ws-7", false));
    }

    /// The daemon's answers to the sync operations, over its files in memory (the daemon's own rules: a push checks
    /// its base at the first piece and its hash at the last, a pull serves chunks of the file).
    fn station_answer(
        files: &Mutex<BTreeMap<String, Vec<u8>>>,
        partial: &mut BTreeMap<String, Vec<u8>>,
        op: SyncOp,
    ) -> Result<Value, String> {
        let mut files = files.lock().unwrap();
        match op {
            SyncOp::Manifest => {
                let entries: Vec<SyncEntry> = files
                    .iter()
                    .map(|(p, b)| SyncEntry {
                        path: p.clone(),
                        hash: sha(b),
                        size: b.len() as u64,
                    })
                    .collect();
                Ok(serde_json::to_value(SyncManifest { entries }).unwrap())
            }
            SyncOp::Pull { path, hash, offset } => {
                let bytes = files.get(&path).ok_or("no such file")?;
                if sha(bytes) != hash {
                    return Err("changed since the scan".into());
                }
                let size = bytes.len() as u64;
                let offset = offset.unwrap_or(0);
                let len = (size - offset).min(SYNC_CHUNK as u64);
                let chunk = &bytes[offset as usize..(offset + len) as usize];
                Ok(serde_json::to_value(SyncPull {
                    path,
                    hash,
                    size,
                    offset,
                    data_b64: STANDARD.encode(chunk),
                    eof: offset + len >= size,
                })
                .unwrap())
            }
            SyncOp::Push {
                path,
                hash,
                data_b64,
                base_hash,
                offset,
                total,
            } => {
                let data = STANDARD.decode(data_b64).map_err(|e| e.to_string())?;
                let offset = offset.unwrap_or(0);
                let total = total.unwrap_or(data.len() as u64);
                if offset == 0 {
                    if files.get(&path).map(|b| sha(b)) != base_hash {
                        return Err(format!("conflict: {path} changed on the station"));
                    }
                    partial.insert(path.clone(), Vec::new());
                }
                let buf = partial.get_mut(&path).ok_or("no partial copy")?;
                if buf.len() as u64 != offset {
                    return Err("out of order".into());
                }
                buf.extend_from_slice(&data);
                let received = buf.len() as u64;
                if received < total {
                    return Ok(serde_json::to_value(SyncPushed {
                        path,
                        received,
                        hash: None,
                    })
                    .unwrap());
                }
                let whole = partial.remove(&path).unwrap();
                if sha(&whole) != hash {
                    return Err("the content does not hash to its hash".into());
                }
                files.insert(path.clone(), whole);
                Ok(serde_json::to_value(SyncPushed {
                    path,
                    received,
                    hash: Some(hash),
                })
                .unwrap())
            }
            SyncOp::Delete { path, base_hash } => {
                match files.get(&path) {
                    Some(bytes) if sha(bytes) == base_hash => {
                        files.remove(&path);
                    }
                    Some(_) => return Err("conflict".into()),
                    None => {}
                }
                Ok(Value::Null)
            }
            SyncOp::Status | SyncOp::Resolve { .. } => Err("not simulated".into()),
        }
    }

    /// Answers the hub's requests on the daemon's end of a live link until the link ends.
    fn serve_station(
        daemon: super::super::link::tests::Daemon,
        files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            let mut partial = BTreeMap::new();
            while let Ok(text) = daemon.hears.recv_timeout(Duration::from_secs(10)) {
                let Ok(HubFrame::Request { id, op }) = serde_json::from_str::<HubFrame>(&text)
                else {
                    continue;
                };
                let answer = match op {
                    Op::Sync(op) => station_answer(&files, &mut partial, op),
                    Op::Facts => {
                        Ok(serde_json::to_value(super::super::link::tests::facts()).unwrap())
                    }
                    other => Err(format!("not simulated: {other:?}")),
                };
                let frame = match answer {
                    Ok(result) => StationFrame::Response {
                        id,
                        ok: true,
                        result: Some(result),
                        error: None,
                    },
                    Err(error) => StationFrame::Response {
                        id,
                        ok: false,
                        result: None,
                        error: Some(error),
                    },
                };
                daemon.say(&frame);
            }
        })
    }

    #[test]
    fn a_file_larger_than_a_chunk_moves_both_ways_over_a_live_link() {
        use super::super::link::tests::{Accept, greet, start};
        let (link, daemon, handle) = start("ws-7", Arc::new(Accept));
        greet(&link, &daemon);
        let big: Vec<u8> = (0..SYNC_CHUNK + 1000).map(|i| (i % 241) as u8).collect();
        let files = Arc::new(Mutex::new(BTreeMap::from([
            ("big.bin".to_string(), big.clone()),
            ("notes.txt".to_string(), b"small".to_vec()),
        ])));
        let served = serve_station(daemon, Arc::clone(&files));
        let hub = Hub::default();
        let remote = LinkRemote(&link);

        let out = reconcile(
            "ws-7",
            "desk-1",
            &remote,
            &hub,
            &hub,
            NOW,
            Limits::default(),
        )
        .unwrap();
        assert_eq!((out.pulled, out.errors.len()), (2, 0), "{:?}", out.errors);
        assert_eq!(
            hub.get(&sha(&big)).unwrap(),
            big,
            "the chunks joined back into the file"
        );

        let edited: Vec<u8> = big.iter().map(|b| b.wrapping_add(1)).collect();
        hub_write(&hub, "big.bin", &edited);
        let out = reconcile(
            "ws-7",
            "desk-1",
            &remote,
            &hub,
            &hub,
            NOW,
            Limits::default(),
        )
        .unwrap();
        assert_eq!((out.pushed, out.errors.len()), (1, 0), "{:?}", out.errors);
        assert_eq!(
            files.lock().unwrap()["big.bin"],
            edited,
            "the station has the hub's edit, pushed in pieces"
        );

        let idle = reconcile(
            "ws-7",
            "desk-1",
            &remote,
            &hub,
            &hub,
            NOW,
            Limits::default(),
        )
        .unwrap();
        assert_eq!(idle, Outcome::default());
        link.kill(1000, "done");
        assert!(handle.join().unwrap().is_ok());
        served.join().unwrap();
    }
}
