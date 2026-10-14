//! Workstation recordings (docs/design/workstations.md, section 7): what happened on a workstation, per lease.
//!
//! A lease on a station that records (`StationSettings::record`) gets a recording when it starts (`begin`) and
//! ends with it (`end`). Its actions are events of the index (`db::recordings`: Postgres, or the state directory
//! without it). Its frames are JPEG objects in the object store (`deliverables::Store`: Garage, or the state folder
//! without it) under `recordings/<station>/<recording>/<seq>.jpg`, seq counting from 1. The index row is written when
//! the recording starts, every `PERSIST_EVERY` frames, and when it ends.
//!
//! - The sampler (`spawn_sampler`) grabs a screen frame of each pixel lease that has a recording, at the station's
//!   `record_fps` (at most `FPS_CAP`; 0 keeps the actions only). A shared lease is the owner's own session, so it is
//!   never sampled: it records its actions.
//! - A takeover: the owner's input on a session (`on_owner_input`) marks the recording `takeover`, and for
//!   `TAKEOVER_HOLD_MS` after the owner's last input the sampler skips its frames, unless the station records
//!   takeovers (`record_takeovers`).
//! - The sweep (`spawn_sweep`) removes the recordings older than their station's `retention_days`, the rows of a
//!   crash's open ones included. The index goes first, then the frames: a frame the index no longer names is an
//!   orphan, and nothing reads it.
//! - Nothing here fails a lease. A recording that cannot be written is logged, and the lease goes on without it.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use apz_station::{ImageFormat, Op, StationSettings};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::leases::{Kind, LeaseView, State};
use super::ops::{self, Failure};
use crate::db::Db;
use crate::settings::RecordingDefaults;
use crate::state::App;

/// The most frames a second a recording keeps.
pub const FPS_CAP: u8 = 5;
/// The most days a station's retention may be.
pub const MAX_RETENTION_DAYS: u32 = 3650;
/// The frames' scale: the sampler asks the daemon for the screen at this fraction of its size.
const FRAME_SCALE: f32 = 0.5;
/// How long after the owner's last input a session counts as taken over.
pub const TAKEOVER_HOLD_MS: u64 = 15_000;
/// The index row is written again every this many frames, so a crash loses at most this many frames' records.
const PERSIST_EVERY: u64 = 10;
/// How many of a recording's actions its detail shows (the newest).
pub const DETAIL_EVENTS: i64 = 1000;
/// How often the sampler looks at the open recordings.
const SAMPLE_TICK: Duration = Duration::from_millis(100);
/// How long one frame may take to come back from the station.
const FRAME_TIMEOUT: Duration = Duration::from_secs(5);
/// The first retention sweep runs this long after start; then one every `SWEEP_EVERY`.
const SWEEP_FIRST: Duration = Duration::from_secs(60);
const SWEEP_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
const DAY_MS: u64 = 86_400_000;

fn now() -> u64 {
    apz_plaza::now_ms()
}

fn server_error(message: impl ToString) -> Failure {
    (500, message.to_string())
}

/// Where frames are kept: the object store (Garage, or the state folder without it).
pub trait Objects: Send + Sync {
    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), String>;
    fn get(&self, key: &str) -> Result<Vec<u8>, String>;
    fn delete(&self, key: &str) -> Result<(), String>;
}

impl Objects for crate::deliverables::Store {
    fn put(&self, key: &str, bytes: &[u8]) -> Result<(), String> {
        self.put_object(key, bytes)
    }

    fn get(&self, key: &str) -> Result<Vec<u8>, String> {
        self.get_object(key)
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        self.delete_object(key)
    }
}

/// A recording: one recorded lease. `frames` counts the frame objects (seq 1 to `frames`) and `actions` the events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recording {
    /// `r<ms>-<n>`.
    pub id: String,
    pub station: String,
    /// The lease it records (`l<seq>`).
    pub lease: String,
    pub agent: u64,
    pub task: Option<String>,
    pub started_ms: u64,
    /// None while the lease is open (and for a row a crash left open).
    pub ended_ms: Option<u64>,
    pub frames: u64,
    pub actions: u64,
    /// The frames' bytes, in the object store.
    pub bytes: u64,
    /// The owner took over the session during the recording.
    pub takeover: bool,
    /// The size of the last frame.
    pub width: Option<u64>,
    pub height: Option<u64>,
}

/// One action of a recording: `kind` is the action's `kind`, and `summary` is the action as it was asked for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub at_ms: u64,
    pub kind: String,
    pub ok: bool,
    pub summary: Value,
}

/// A recording with its actions, for the owner's detail view.
#[derive(Debug, Serialize)]
pub struct Detail {
    #[serde(flatten)]
    pub recording: Recording,
    pub events: Vec<Event>,
}

/// An open recording: its index row as it stands, and until when the owner drives its session.
#[derive(Debug, Clone)]
struct Active {
    rec: Recording,
    /// The owner drives the session until this (ms); 0 when not.
    takeover_until: u64,
}

/// The open recordings, by lease. A lease's recording is open while the lease is.
#[derive(Default)]
pub struct Recordings {
    open: Mutex<BTreeMap<String, Active>>,
    seq: AtomicU64,
}

impl Recordings {
    fn guard(&self) -> MutexGuard<'_, BTreeMap<String, Active>> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether the lease has a recording open.
    pub fn is_open(&self, lease: &str) -> bool {
        self.guard().contains_key(lease)
    }

    /// Whether the recording with this id is open.
    pub fn is_open_recording(&self, id: &str) -> bool {
        self.guard().values().any(|a| a.rec.id == id)
    }

    /// The leases that have a recording open.
    pub fn open_leases(&self) -> Vec<String> {
        self.guard().keys().cloned().collect()
    }

    /// Whether the owner drives the lease's session at `now`.
    pub fn taking_over(&self, lease: &str, now: u64) -> bool {
        self.guard().get(lease).is_some_and(|a| a.takeover_until > now)
    }

    /// `rec` with the live counts of its recording when that is open.
    pub fn live(&self, rec: Recording) -> Recording {
        match self.guard().values().find(|a| a.rec.id == rec.id) {
            Some(active) => active.rec.clone(),
            None => rec,
        }
    }

    /// Opens the recording of a lease on `station`, and writes its index row. None when the station does not record.
    /// A lease that has a recording open keeps it.
    pub fn begin(&self, db: &Db, settings: &StationSettings, station: &str, lease: &LeaseView, now: u64) -> Result<Option<String>, String> {
        if !settings.record {
            return Ok(None);
        }
        let lease = &lease.lease;
        let mut guard = self.guard();
        if let Some(active) = guard.get(&lease.id) {
            return Ok(Some(active.rec.id.clone()));
        }
        let id = format!("r{now}-{}", self.seq.fetch_add(1, Ordering::Relaxed) + 1);
        let rec = Recording {
            id: id.clone(),
            station: station.to_string(),
            lease: lease.id.clone(),
            agent: lease.agent,
            task: lease.task.clone(),
            started_ms: now,
            ended_ms: None,
            frames: 0,
            actions: 0,
            bytes: 0,
            takeover: false,
            width: None,
            height: None,
        };
        db.put_recording(&rec)?;
        guard.insert(lease.id.clone(), Active { rec, takeover_until: 0 });
        Ok(Some(id))
    }

    /// Adds an action to the lease's recording, when one is open on `station`. False when none is.
    pub fn action(&self, db: &Db, station: &str, lease: &str, summary: &Value, ok: bool, now: u64) -> Result<bool, String> {
        let mut guard = self.guard();
        let Some(active) = guard.get_mut(lease).filter(|a| a.rec.station == station) else {
            return Ok(false);
        };
        let event = Event { at_ms: now, kind: summary["kind"].as_str().unwrap_or("action").to_string(), ok, summary: summary.clone() };
        db.append_recording_event(&active.rec.id, &event)?;
        active.rec.actions += 1;
        Ok(true)
    }

    /// Stores a frame of the lease's recording, when one is open on `station`, as its next seq. False when none is.
    /// The object is stored without the lock held (it is a network write); the sampler is the only writer of frames.
    #[allow(clippy::too_many_arguments)]
    pub fn frame(&self, db: &Db, objects: &dyn Objects, station: &str, lease: &str, jpeg: &[u8], w: u32, h: u32) -> Result<bool, String> {
        let (id, seq) = {
            let guard = self.guard();
            let Some(active) = guard.get(lease).filter(|a| a.rec.station == station) else {
                return Ok(false);
            };
            (active.rec.id.clone(), active.rec.frames + 1)
        };
        objects.put(&frame_key(station, &id, seq), jpeg)?;
        let mut guard = self.guard();
        let Some(active) = guard.get_mut(lease).filter(|a| a.rec.id == id) else {
            // The lease ended while the frame was stored: the frame is not part of the recording.
            let _ = objects.delete(&frame_key(station, &id, seq));
            return Ok(false);
        };
        active.rec.frames = seq;
        active.rec.bytes += jpeg.len() as u64;
        active.rec.width = Some(u64::from(w));
        active.rec.height = Some(u64::from(h));
        if seq % PERSIST_EVERY == 0 {
            db.put_recording(&active.rec)?;
        }
        Ok(true)
    }

    /// Marks the lease's recording as taken over, and holds its frames while the owner drives. False when the lease
    /// has no recording open.
    pub fn owner_input(&self, lease: &str, now: u64) -> bool {
        match self.guard().get_mut(lease) {
            Some(active) => {
                active.rec.takeover = true;
                active.takeover_until = now + TAKEOVER_HOLD_MS;
                true
            }
            None => false,
        }
    }

    /// Closes the lease's recording at `now` on `station`: its index row is written with the end and the counts.
    /// The recording leaves the open set even when the row cannot be written (a crash row is swept later).
    pub fn end(&self, db: &Db, station: &str, lease: &str, now: u64) -> Result<Option<Recording>, String> {
        let mut guard = self.guard();
        let Some(active) = guard.get(lease).filter(|a| a.rec.station == station) else {
            return Ok(None);
        };
        let mut rec = active.rec.clone();
        rec.ended_ms = Some(now);
        let written = db.put_recording(&rec);
        guard.remove(lease);
        written.map(|()| Some(rec))
    }
}

/// The object key of one frame of a recording.
pub fn frame_key(station: &str, recording: &str, seq: u64) -> String {
    format!("recordings/{station}/{recording}/{seq}.jpg")
}

/// The JPEG and its size from a screenshot's answer (`{"w", "h", "format", "data_b64"}`).
fn decode_frame(answer: &Value) -> Result<(Vec<u8>, u32, u32), String> {
    if answer["format"].as_str() != Some("jpeg") {
        return Err("the screenshot is not a JPEG".into());
    }
    let data = answer["data_b64"].as_str().ok_or("the screenshot names no data_b64")?;
    let jpeg = base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| format!("the screenshot is not base64: {e}"))?;
    let w = answer["w"].as_u64().unwrap_or(0) as u32;
    let h = answer["h"].as_u64().unwrap_or(0) as u32;
    Ok((jpeg, w, h))
}

/// The settings a station joins with: the hub's recording defaults over the station's own defaults.
pub fn join_settings(defaults: &RecordingDefaults) -> StationSettings {
    StationSettings {
        record: defaults.record,
        retention_days: defaults.retention_days,
        record_fps: defaults.record_fps,
        record_takeovers: defaults.record_takeovers,
        ..StationSettings::default()
    }
}

/// Deletes the frame objects of a recording. A failed delete is logged and counted: the object is then an orphan
/// that nothing names.
pub fn delete_frames(objects: &dyn Objects, rec: &Recording) -> usize {
    (1..=rec.frames)
        .filter(|seq| match objects.delete(&frame_key(&rec.station, &rec.id, *seq)) {
            Ok(()) => false,
            Err(reason) => {
                eprintln!("apid: recording {}: frame {seq} was not deleted: {reason}", rec.id);
                true
            }
        })
        .count()
}

/// The owner's delete, index part: the row goes, and an open recording is refused. Returns the row, for its frames.
fn remove_index(db: &Db, recordings: &Recordings, id: &str) -> Result<Recording, Failure> {
    let rec = db.recording(id).map_err(server_error)?.ok_or((404, "no such recording".to_string()))?;
    if recordings.is_open_recording(id) {
        return Err((409, "this recording is still open: its lease has not ended".into()));
    }
    db.delete_recording(id).map_err(server_error)?;
    Ok(rec)
}

/// The retention sweep over `stations` (id and retention in days): every recording that ended before its station's
/// cutoff goes, and so does a row a crash left open (one no lease holds now). Returns how many went.
fn sweep_with(db: &Db, objects: &dyn Objects, recordings: &Recordings, stations: &[(String, u32)], now: u64) -> usize {
    let mut removed = 0;
    for (station, days) in stations {
        let cutoff = now.saturating_sub(u64::from(*days) * DAY_MS);
        let expired = match db.recordings_expired(station, cutoff) {
            Ok(rows) => rows,
            Err(reason) => {
                eprintln!("apid: the recording sweep of workstation {station} did not read its index: {reason}");
                continue;
            }
        };
        for rec in expired {
            if recordings.is_open_recording(&rec.id) {
                continue;
            }
            if let Err(reason) = db.delete_recording(&rec.id) {
                eprintln!("apid: recording {} was not removed: {reason}", rec.id);
                continue;
            }
            delete_frames(objects, &rec);
            removed += 1;
        }
    }
    removed
}

/// Opens the recording of a lease that starts on `station`, if the station records. Logged when it cannot be written.
pub fn begin(app: &App, station: &str, lease: &LeaseView) {
    let Some(settings) = app.stations.station(station).map(|s| s.settings) else {
        return;
    };
    if let Err(reason) = app.stations.recordings.begin(&app.db, &settings, station, lease, now()) {
        eprintln!("apid: workstation {station}: the recording of lease {} did not start: {reason}", lease.lease.id);
    }
}

/// Closes the recording of a lease that ended on `station`. Logged when its row cannot be written.
pub fn end(app: &App, station: &str, lease: &str) {
    if let Err(reason) = app.stations.recordings.end(&app.db, station, lease, now()) {
        eprintln!("apid: workstation {station}: the recording of lease {lease} was not closed: {reason}");
    }
}

/// Appends an action to the lease's recording, when one is open. A lease with no recording (or none given) is a no-op.
/// The action path calls it; until that lands, nothing does.
#[allow(dead_code)]
pub fn on_action(app: &App, station: &str, lease: Option<&str>, summary: &Value, ok: bool) {
    let Some(lease) = lease else {
        return;
    };
    if let Err(reason) = app.stations.recordings.action(&app.db, station, lease, summary, ok, now()) {
        eprintln!("apid: workstation {station}: an action of lease {lease} was not recorded: {reason}");
    }
}

/// Stores a frame in the lease's recording, when one is open. `jpeg` is the whole image; `w` and `h` its size.
pub fn on_frame(app: &App, station: &str, lease: Option<&str>, jpeg: &[u8], w: u32, h: u32) {
    let Some(lease) = lease else {
        return;
    };
    if let Err(reason) = app.stations.recordings.frame(&app.db, &*app.deliverables, station, lease, jpeg, w, h) {
        eprintln!("apid: workstation {station}: a frame of lease {lease} was not recorded: {reason}");
    }
}

/// The owner's input reached a session of `lease`: its recording is marked as a takeover.
pub fn on_owner_input(app: &App, lease: &str) {
    app.stations.recordings.owner_input(lease, now());
}

/// The recordings, newest first: one station's when given, those started before `before` (ms) when given. An open
/// recording carries its live counts.
pub fn list(app: &App, station: Option<&str>, before: Option<u64>, limit: i64) -> Result<Vec<Recording>, String> {
    let rows = app.db.recordings(station, before, limit)?;
    Ok(rows.into_iter().map(|rec| app.stations.recordings.live(rec)).collect())
}

fn recording(app: &App, id: &str) -> Result<Option<Recording>, String> {
    Ok(app.db.recording(id)?.map(|rec| app.stations.recordings.live(rec)))
}

/// One recording with its newest actions; None when there is no such recording.
pub fn detail(app: &App, id: &str) -> Result<Option<Detail>, String> {
    let Some(rec) = recording(app, id)? else {
        return Ok(None);
    };
    let events = app.db.recording_events(id, DETAIL_EVENTS)?;
    Ok(Some(Detail { recording: rec, events }))
}

/// One frame of a recording, as stored. None when the recording has no frame `seq` (they count from 1).
pub fn frame(app: &App, id: &str, seq: u64) -> Result<Option<Vec<u8>>, String> {
    let Some(rec) = recording(app, id)? else {
        return Ok(None);
    };
    if seq == 0 || seq > rec.frames {
        return Ok(None);
    }
    app.deliverables.get_object(&frame_key(&rec.station, id, seq)).map(Some)
}

/// The owner deletes a recording. Its row goes at once; its frames are deleted in the background.
pub fn remove(app: &App, id: &str, actor: &str) -> Result<Recording, Failure> {
    let rec = remove_index(&app.db, &app.stations.recordings, id)?;
    let objects = Arc::clone(&app.deliverables);
    let frames = rec.clone();
    std::thread::spawn(move || {
        delete_frames(&*objects, &frames);
    });
    ops::audit(
        app,
        Some(&rec.station),
        None,
        "recording_delete",
        actor,
        json!({"recording": rec.id, "lease": rec.lease, "frames": rec.frames}),
    )
    .map_err(server_error)?;
    Ok(rec)
}

/// Stores the hub's recording defaults, for the stations that join from now on. A value out of range is refused.
pub fn set_defaults(app: &App, defaults: RecordingDefaults, actor: &str) -> Result<RecordingDefaults, Failure> {
    if defaults.retention_days > MAX_RETENTION_DAYS {
        return Err((400, format!("retention_days is 0 to {MAX_RETENTION_DAYS}")));
    }
    if defaults.record_fps > FPS_CAP {
        return Err((400, format!("record_fps is 0 to {FPS_CAP}")));
    }
    let mut settings = app.settings.get();
    settings.workstation_recordings = defaults;
    app.settings.put(settings).map_err(|reason| (400, reason))?;
    ops::audit(app, None, None, "recording_defaults", actor, json!({"defaults": defaults})).map_err(server_error)?;
    Ok(defaults)
}

/// One retention sweep over the hub's stations, as of `now`. Returns how many recordings it removed.
pub fn sweep(app: &App, now: u64) -> usize {
    let stations: Vec<(String, u32)> = app.stations.list().into_iter().map(|s| (s.id, s.settings.retention_days)).collect();
    sweep_with(&app.db, &*app.deliverables, &app.stations.recordings, &stations, now)
}

/// One pass of the sampler: a frame of each pixel lease that has a recording, on a station that records, once its
/// interval has passed. A takeover holds the frames unless the station records takeovers.
fn sample_once(app: &App, last: &mut BTreeMap<String, u64>, now: u64) {
    let open = app.stations.recordings.open_leases();
    last.retain(|lease, _| open.contains(lease));
    if app.control.paused() {
        return;
    }
    for station in app.stations.list() {
        let settings = &station.settings;
        if station.revoked || !settings.record || settings.record_fps == 0 {
            continue;
        }
        let Some(link) = app.stations.live(&station.id) else {
            continue;
        };
        let interval = 1000 / u64::from(settings.record_fps.min(FPS_CAP));
        for view in app.stations.leases.views(&station.id) {
            let lease = &view.lease;
            let session = match (lease.kind, lease.state, lease.session) {
                (Kind::Pixel, State::Active, Some(session)) => session,
                _ => continue,
            };
            if !app.stations.recordings.is_open(&lease.id) {
                continue;
            }
            if last.get(&lease.id).is_some_and(|at| now < at + interval) {
                continue;
            }
            if !settings.record_takeovers && app.stations.recordings.taking_over(&lease.id, now) {
                continue;
            }
            last.insert(lease.id.clone(), now);
            let grab = Op::Screenshot { session, scale: FRAME_SCALE, grid: false, format: ImageFormat::Jpeg };
            match link.request(grab, FRAME_TIMEOUT).and_then(|answer| decode_frame(&answer)) {
                Ok((jpeg, w, h)) => on_frame(app, &station.id, Some(&lease.id), &jpeg, w, h),
                Err(reason) => eprintln!("apid: workstation {}: no frame of lease {}: {reason}", station.id, lease.id),
            }
        }
    }
}

/// Starts the sampler: a thread that grabs the frames of the recorded pixel leases.
pub fn spawn_sampler(app: &Arc<App>) {
    let weak = app.handle();
    std::thread::spawn(move || {
        let mut last: BTreeMap<String, u64> = BTreeMap::new();
        loop {
            std::thread::sleep(SAMPLE_TICK);
            let Some(app) = weak.upgrade() else {
                return;
            };
            sample_once(&app, &mut last, now());
        }
    });
}

/// Starts the retention sweep: a first pass a minute after start, then one every six hours.
pub fn spawn_sweep(app: &Arc<App>) {
    let weak = app.handle();
    std::thread::spawn(move || {
        std::thread::sleep(SWEEP_FIRST);
        loop {
            let Some(app) = weak.upgrade() else {
                return;
            };
            let removed = sweep(&app, now());
            drop(app);
            if removed > 0 {
                eprintln!("apid: workstation recordings: the retention sweep removed {removed}");
            }
            std::thread::sleep(SWEEP_EVERY);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stations::leases::Lease;
    use std::path::{Path, PathBuf};

    /// Frames kept in memory, by key.
    #[derive(Default)]
    struct MemObjects(Mutex<BTreeMap<String, Vec<u8>>>);

    impl MemObjects {
        fn has(&self, key: &str) -> bool {
            self.0.lock().unwrap().contains_key(key)
        }
    }

    impl Objects for MemObjects {
        fn put(&self, key: &str, bytes: &[u8]) -> Result<(), String> {
            self.0.lock().unwrap().insert(key.to_string(), bytes.to_vec());
            Ok(())
        }

        fn get(&self, key: &str) -> Result<Vec<u8>, String> {
            self.0.lock().unwrap().get(key).cloned().ok_or_else(|| format!("no object {key}"))
        }

        fn delete(&self, key: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }

    /// A fresh state directory for one test's file backend.
    fn state_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apid-recordings-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A pixel lease `id` of `agent` on station w1, active on session 9.
    fn view(id: &str, agent: u64) -> LeaseView {
        LeaseView {
            lease: Lease {
                id: id.into(),
                station: "w1".into(),
                agent,
                task: Some("render".into()),
                kind: Kind::Pixel,
                session: Some(9),
                started_ms: 1000,
                state: State::Active,
            },
            position: None,
        }
    }

    fn recording_of(id: &str, station: &str, lease: &str, started_ms: u64) -> Recording {
        Recording {
            id: id.into(),
            station: station.into(),
            lease: lease.into(),
            agent: 1,
            task: None,
            started_ms,
            ended_ms: None,
            frames: 0,
            actions: 0,
            bytes: 0,
            takeover: false,
            width: None,
            height: None,
        }
    }

    #[test]
    fn a_lease_is_recorded_from_its_start_to_its_end_and_survives_a_restart() {
        let dir = state_dir("lifecycle");
        let db = Db::files_only(&dir);
        let objects = MemObjects::default();
        let recs = Recordings::default();
        let settings = StationSettings::default();

        let id = recs.begin(&db, &settings, "w1", &view("l1", 7), 1000).unwrap().expect("the station records");
        assert!(recs.is_open("l1"));
        assert_eq!(db.recording(&id).unwrap().map(|r| r.ended_ms), Some(None), "the row is written at the start");

        assert!(recs.action(&db, "w1", "l1", &json!({"kind": "click", "x": 3}), true, 1100).unwrap());
        assert!(!recs.action(&db, "w1", "l2", &json!({"kind": "click"}), true, 1100).unwrap(), "no recording for another lease");
        assert!(!recs.action(&db, "w2", "l1", &json!({"kind": "click"}), true, 1100).unwrap(), "not on another station");

        assert!(recs.frame(&db, &objects, "w1", "l1", b"one", 1280, 720).unwrap());
        assert!(recs.frame(&db, &objects, "w1", "l1", b"four", 1280, 720).unwrap());
        assert_eq!(objects.get(&frame_key("w1", &id, 1)), Ok(b"one".to_vec()), "seq counts from 1");
        assert_eq!(objects.get(&frame_key("w1", &id, 2)), Ok(b"four".to_vec()));

        assert_eq!(recs.end(&db, "w2", "l1", 1400).unwrap(), None, "a station cannot end another's lease");
        assert!(recs.is_open("l1"));
        let ended = recs.end(&db, "w1", "l1", 1500).unwrap().expect("ended");
        assert_eq!(
            ended,
            Recording {
                id: id.clone(),
                station: "w1".into(),
                lease: "l1".into(),
                agent: 7,
                task: Some("render".into()),
                started_ms: 1000,
                ended_ms: Some(1500),
                frames: 2,
                actions: 1,
                bytes: 7,
                takeover: false,
                width: Some(1280),
                height: Some(720),
            }
        );
        assert!(!recs.is_open("l1"));
        assert!(!recs.frame(&db, &objects, "w1", "l1", b"late", 1, 1).unwrap(), "a frame after the end is not kept");
        assert!(!objects.has(&frame_key("w1", &id, 3)));

        assert_eq!(db.recording(&id).unwrap(), Some(ended.clone()));
        assert_eq!(db.recordings(Some("w1"), None, 10).unwrap(), vec![ended.clone()]);
        let events = db.recording_events(&id, 10).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!((events[0].kind.as_str(), events[0].ok, events[0].at_ms), ("click", true, 1100));

        let reopened = Db::files_only(&dir);
        assert_eq!(reopened.recording(&id).unwrap(), Some(ended), "the index is read back from the files");
        assert_eq!(reopened.recording_events(&id, 10).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_station_that_does_not_record_begins_nothing() {
        let dir = state_dir("off");
        let db = Db::files_only(&dir);
        let recs = Recordings::default();
        let off = StationSettings { record: false, ..StationSettings::default() };
        assert_eq!(recs.begin(&db, &off, "w1", &view("l1", 7), 1000).unwrap(), None);
        assert!(!recs.is_open("l1"));
        assert!(!recs.action(&db, "w1", "l1", &json!({"kind": "click"}), true, 1100).unwrap());
        assert!(db.recordings(None, None, 10).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn frames_are_persisted_every_few_so_a_crash_keeps_their_count() {
        let dir = state_dir("persist");
        let db = Db::files_only(&dir);
        let objects = MemObjects::default();
        let recs = Recordings::default();
        let id = recs.begin(&db, &StationSettings::default(), "w1", &view("l1", 7), 1000).unwrap().unwrap();
        for _ in 0..PERSIST_EVERY {
            recs.frame(&db, &objects, "w1", "l1", b"f", 2, 2).unwrap();
        }
        assert_eq!(db.recording(&id).unwrap().map(|r| r.frames), Some(PERSIST_EVERY), "the row has the tenth frame while open");
        recs.frame(&db, &objects, "w1", "l1", b"f", 2, 2).unwrap();
        assert_eq!(db.recording(&id).unwrap().map(|r| r.frames), Some(PERSIST_EVERY), "the eleventh is in memory until the next write");
        assert_eq!(recs.live(db.recording(&id).unwrap().unwrap()).frames, PERSIST_EVERY + 1, "the list shows the live count");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_owner_takeover_is_marked_and_holds_the_frames_while_the_owner_drives() {
        let dir = state_dir("takeover");
        let db = Db::files_only(&dir);
        let recs = Recordings::default();
        recs.begin(&db, &StationSettings::default(), "w1", &view("l1", 7), 1000).unwrap();
        assert!(!recs.owner_input("l9", 2000), "no recording, no takeover");
        assert!(recs.owner_input("l1", 2000));
        assert!(recs.taking_over("l1", 2000));
        assert!(recs.taking_over("l1", 2000 + TAKEOVER_HOLD_MS - 1));
        assert!(!recs.taking_over("l1", 2000 + TAKEOVER_HOLD_MS), "the hold ends at its last moment");
        let ended = recs.end(&db, "w1", "l1", 3000).unwrap().unwrap();
        assert!(ended.takeover);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn decode_frame_reads_a_jpeg_answer_and_refuses_the_rest() {
        let answer = json!({"w": 4, "h": 2, "format": "jpeg", "data_b64": "YWJj"});
        assert_eq!(decode_frame(&answer), Ok((b"abc".to_vec(), 4, 2)));
        assert!(decode_frame(&json!({"w": 4, "h": 2, "format": "png", "data_b64": "YWJj"})).is_err());
        assert!(decode_frame(&json!({"format": "jpeg"})).is_err(), "no data");
    }

    #[test]
    fn the_owners_delete_refuses_an_open_recording_then_removes_the_row_and_its_frames() {
        let dir = state_dir("delete");
        let db = Db::files_only(&dir);
        let objects = MemObjects::default();
        let recs = Recordings::default();
        let id = recs.begin(&db, &StationSettings::default(), "w1", &view("l1", 7), 1000).unwrap().unwrap();
        recs.frame(&db, &objects, "w1", "l1", b"a", 1, 1).unwrap();
        recs.frame(&db, &objects, "w1", "l1", b"b", 1, 1).unwrap();
        recs.action(&db, "w1", "l1", &json!({"kind": "wait"}), true, 1100).unwrap();

        assert_eq!(remove_index(&db, &recs, "nope").map_err(|e| e.0), Err(404));
        assert_eq!(remove_index(&db, &recs, &id).map_err(|e| e.0), Err(409), "the lease has not ended");
        recs.end(&db, "w1", "l1", 1500).unwrap();

        let rec = remove_index(&db, &recs, &id).unwrap();
        assert_eq!(rec.frames, 2);
        assert_eq!(db.recording(&id).unwrap(), None);
        assert!(db.recording_events(&id, 10).unwrap().is_empty());
        assert_eq!(delete_frames(&objects, &rec), 0);
        assert!(!objects.has(&frame_key("w1", &id, 1)) && !objects.has(&frame_key("w1", &id, 2)));
        assert_eq!(remove_index(&db, &recs, &id).map_err(|e| e.0), Err(404), "a second delete finds nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_retention_sweep_removes_only_the_expired_and_never_an_open_recording() {
        let dir = state_dir("sweep");
        let db = Db::files_only(&dir);
        let objects = MemObjects::default();
        let recs = Recordings::default();
        let day = |n: u64| n * DAY_MS;
        let w1 = StationSettings { retention_days: 1, ..StationSettings::default() };
        let w2 = StationSettings { retention_days: 30, ..StationSettings::default() };

        // a: ended on day 2, past w1's cutoff (day 9 of 10). It has a frame.
        let a = recs.begin(&db, &w1, "w1", &view("la", 1), day(1)).unwrap().unwrap();
        recs.frame(&db, &objects, "w1", "la", b"frame", 1, 1).unwrap();
        recs.end(&db, "w1", "la", day(2)).unwrap();
        // b: ended on day 9.5, inside w1's window.
        let b = recs.begin(&db, &w1, "w1", &view("lb", 1), day(8)).unwrap().unwrap();
        recs.end(&db, "w1", "lb", day(9) + DAY_MS / 2).unwrap();
        // c: still open since day 1: a lease holds it, however old it is.
        let c = recs.begin(&db, &w1, "w1", &view("lc", 1), day(1)).unwrap().unwrap();
        // d: ended on day 2 on w2, whose window is 30 days.
        let d = recs
            .begin(&db, &w2, "w2", &LeaseView { lease: Lease { station: "w2".into(), ..view("ld", 1).lease }, position: None }, day(1))
            .unwrap()
            .unwrap();
        recs.end(&db, "w2", "ld", day(2)).unwrap();
        // e: a crash's row: open in the index, held by no lease.
        let e = "re-crash".to_string();
        db.put_recording(&recording_of(&e, "w1", "lx", day(2))).unwrap();

        let stations = vec![("w1".to_string(), 1), ("w2".to_string(), 30)];
        assert_eq!(sweep_with(&db, &objects, &recs, &stations, day(10)), 2, "a and the crash row");
        assert_eq!(db.recording(&a).unwrap(), None);
        assert!(!objects.has(&frame_key("w1", &a, 1)), "a's frame goes with it");
        assert_eq!(db.recording(&e).unwrap(), None);
        assert!(db.recording(&b).unwrap().is_some(), "ended inside the window");
        assert!(db.recording(&c).unwrap().is_some(), "open");
        assert!(db.recording(&d).unwrap().is_some(), "w2 keeps thirty days");
        assert_eq!(sweep_with(&db, &objects, &recs, &stations, day(10)), 0, "a second sweep finds nothing new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_joined_station_starts_with_the_hubs_recording_defaults() {
        let defaults = RecordingDefaults { record: false, retention_days: 7, record_fps: 3, record_takeovers: false };
        let joined = join_settings(&defaults);
        assert_eq!((joined.record, joined.retention_days, joined.record_fps, joined.record_takeovers), (false, 7, 3, false));
        assert_eq!(joined.work_folder, StationSettings::default().work_folder, "the rest is the station default");

        let standard = join_settings(&RecordingDefaults::default());
        assert_eq!(standard, StationSettings::default(), "the defaults are the station's own");
    }

    #[test]
    fn a_frame_is_keyed_by_its_station_recording_and_seq() {
        assert_eq!(frame_key("w3", "r1700-2", 12), "recordings/w3/r1700-2/12.jpg");
        assert_eq!(Path::new(&frame_key("w3", "r1", 1)).extension().and_then(|e| e.to_str()), Some("jpg"));
    }
}
