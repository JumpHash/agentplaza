//! A file's one writer: a dedicated thread that owns the file's only write connection.
//!
//! Callers send it operations: closures that receive the connection inside the open transaction and call typed query
//! functions on it. The thread takes every operation waiting when it wakes (up to [`MAX_BATCH`]), runs them in one
//! `BEGIN IMMEDIATE` transaction, each inside its own savepoint (an operation that fails or panics rolls back to its
//! savepoint and fails alone), commits once, and answers each caller with its result and the commit's sequence number.
//! A burst of writes therefore costs one sync to disk, and because there is no second writer, no write ever meets
//! `SQLITE_BUSY`.

use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use rusqlite::{Connection, TransactionBehavior};

use crate::StoreError;

/// The most operations one transaction runs.
pub const MAX_BATCH: usize = 1024;
/// Operations waiting for the writer before a sender blocks (backpressure on a writer that cannot keep up).
const QUEUE: usize = 8192;
/// Quiet time after a write before the writer checkpoints.
const IDLE: Duration = Duration::from_millis(250);
/// Commits in a burst after which the writer checkpoints without waiting to be idle.
const BURST_CHECKPOINT: u64 = 256;
/// A WAL larger than this is truncated (not only checkpointed) when the writer is idle.
const WAL_TRUNCATE_BYTES: u64 = 64 << 20;
/// What `journal_size_limit` keeps of a WAL after it is reset.
const WAL_KEEP_BYTES: i64 = 16 << 20;

/// The table every file keeps its commit sequence in, so sequence numbers only grow, across restarts too, and a
/// backup knows the sequence it holds. The store's own, outside the numbered migrations.
const COMMITS: &str = "CREATE TABLE IF NOT EXISTS store_commit (
    id INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    seq INTEGER NOT NULL CHECK (seq >= 0),
    at_ms INTEGER NOT NULL
) STRICT";

/// The commit sequence recorded in a file (0 when it has none yet).
pub(crate) fn stored_seq(conn: &Connection) -> Result<u64, StoreError> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'store_commit')",
        [],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(0);
    }
    let seq: Option<i64> = conn
        .query_row("SELECT seq FROM store_commit WHERE id = 1", [], |r| {
            r.get(0)
        })
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })?;
    Ok(seq.map_or(0, |s| u64::try_from(s).unwrap_or(0)))
}

type Value = Box<dyn Any + Send>;
type Op = Box<dyn FnOnce(&Connection) -> Result<Value, StoreError> + Send>;
type Reply = Result<(Value, u64), StoreError>;

enum Msg {
    Job {
        op: Op,
        reply: Option<SyncSender<Reply>>,
    },
    Flush(SyncSender<u64>),
    Stop,
}

/// A committed write's result and the sequence number of the commit that holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Written<T> {
    pub value: T,
    pub seq: u64,
}

/// Counters of a writer's work, for tests and the hub's metrics.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WriterStats {
    pub commits: u64,
    pub ops: u64,
    pub failed_ops: u64,
    pub largest_batch: usize,
    pub checkpoints: u64,
}

struct Shared {
    seq: Mutex<u64>,
    advanced: Condvar,
    commits: AtomicU64,
    ops: AtomicU64,
    failed_ops: AtomicU64,
    largest_batch: AtomicUsize,
    checkpoints: AtomicU64,
}

impl Shared {
    fn advance(&self) -> u64 {
        let mut seq = self.seq.lock().unwrap_or_else(PoisonError::into_inner);
        *seq += 1;
        self.advanced.notify_all();
        *seq
    }

    fn get(&self) -> u64 {
        *self.seq.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The writer of one file. Shared by every thread of the hub (`Arc<Writer>` or inside a [`crate::Db`]).
pub struct Writer {
    tx: SyncSender<Msg>,
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
    path: PathBuf,
}

impl Writer {
    /// Starts the writer thread over `conn`, the file's write connection (already migrated).
    pub(crate) fn start(conn: Connection, path: PathBuf) -> Result<Writer, StoreError> {
        conn.pragma_update(None, "journal_size_limit", WAL_KEEP_BYTES)?;
        conn.execute_batch(COMMITS)?;
        conn.execute(
            "INSERT INTO store_commit (id, seq, at_ms) VALUES (1, 0, 0) ON CONFLICT (id) DO NOTHING",
            [],
        )?;
        let seq = stored_seq(&conn)?;
        let (tx, rx) = mpsc::sync_channel(QUEUE);
        let shared = Arc::new(Shared {
            seq: Mutex::new(seq),
            advanced: Condvar::new(),
            commits: AtomicU64::new(0),
            ops: AtomicU64::new(0),
            failed_ops: AtomicU64::new(0),
            largest_batch: AtomicUsize::new(0),
            checkpoints: AtomicU64::new(0),
        });
        let thread_shared = Arc::clone(&shared);
        let thread_path = path.clone();
        let name = format!(
            "store-writer-{}",
            path.file_stem().and_then(|s| s.to_str()).unwrap_or("db")
        );
        let thread = std::thread::Builder::new()
            .name(name)
            .spawn(move || run(conn, rx, &thread_shared, &thread_path))?;
        Ok(Writer {
            tx,
            shared,
            thread: Mutex::new(Some(thread)),
            path,
        })
    }

    /// Runs `op` in the writer's next transaction and waits for its commit. The result carries the commit's sequence
    /// number. An operation that fails rolls back alone: the others in its transaction still commit.
    pub fn write<T, F>(&self, op: F) -> Result<Written<T>, StoreError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
    {
        let (reply, answer) = mpsc::sync_channel(1);
        let op: Op = Box::new(move |conn| op(conn).map(|v| Box::new(v) as Value));
        self.tx
            .send(Msg::Job {
                op,
                reply: Some(reply),
            })
            .map_err(|_| StoreError::Closed)?;
        let (value, seq) = answer.recv().map_err(|_| StoreError::Closed)??;
        let value = value
            .downcast::<T>()
            .map_err(|_| StoreError::invalid("a write returned a value of another type"))?;
        Ok(Written { value: *value, seq })
    }

    /// Queues `op` without waiting for it. A failure is logged with the file's name and counted in
    /// [`WriterStats::failed_ops`]; [`Writer::flush`] waits until it is committed.
    pub fn send<F>(&self, op: F) -> Result<(), StoreError>
    where
        F: FnOnce(&Connection) -> Result<(), StoreError> + Send + 'static,
    {
        let op: Op = Box::new(move |conn| op(conn).map(|()| Box::new(()) as Value));
        self.tx
            .send(Msg::Job { op, reply: None })
            .map_err(|_| StoreError::Closed)
    }

    /// Waits until every operation sent before this call is committed; returns the sequence then committed.
    pub fn flush(&self) -> Result<u64, StoreError> {
        let (reply, answer) = mpsc::sync_channel(1);
        self.tx
            .send(Msg::Flush(reply))
            .map_err(|_| StoreError::Closed)?;
        answer.recv().map_err(|_| StoreError::Closed)
    }

    /// The sequence number of the latest commit (0 before the first).
    pub fn committed(&self) -> u64 {
        self.shared.get()
    }

    /// Waits until commit `seq` has happened (true), or `timeout` passes first (false). A reader on another thread
    /// that must see a write calls this with the write's [`Written::seq`] before it reads.
    pub fn wait_for(&self, seq: u64, timeout: Duration) -> bool {
        let guard = self
            .shared
            .seq
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (guard, _) = self
            .shared
            .advanced
            .wait_timeout_while(guard, timeout, |committed| *committed < seq)
            .unwrap_or_else(PoisonError::into_inner);
        *guard >= seq
    }

    /// Counters of the writer's work so far.
    pub fn stats(&self) -> WriterStats {
        WriterStats {
            commits: self.shared.commits.load(Ordering::Relaxed),
            ops: self.shared.ops.load(Ordering::Relaxed),
            failed_ops: self.shared.failed_ops.load(Ordering::Relaxed),
            largest_batch: self.shared.largest_batch.load(Ordering::Relaxed),
            checkpoints: self.shared.checkpoints.load(Ordering::Relaxed),
        }
    }

    /// The file this writer writes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Commits what is queued, checkpoints and truncates the WAL, and stops the thread. Later writes fail with
    /// [`StoreError::Closed`]. Called by [`crate::Store::close`]; also on drop.
    pub(crate) fn stop(&self) {
        let thread = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(thread) = thread {
            let _ = self.tx.send(Msg::Stop);
            let _ = thread.join();
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(mut conn: Connection, rx: Receiver<Msg>, shared: &Shared, path: &Path) {
    let mut since_checkpoint = 0u64;
    let mut dirty = false;
    loop {
        let first = if dirty {
            match rx.recv_timeout(IDLE) {
                Ok(msg) => msg,
                Err(RecvTimeoutError::Timeout) => {
                    checkpoint(&conn, path, true, shared);
                    since_checkpoint = 0;
                    dirty = false;
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(msg) => msg,
                Err(_) => break,
            }
        };
        let mut jobs = Vec::new();
        let mut flushes = Vec::new();
        let mut stop = false;
        let mut next = Some(first);
        while let Some(msg) = next.take() {
            match msg {
                Msg::Job { op, reply } => jobs.push((op, reply)),
                Msg::Flush(reply) => flushes.push(reply),
                Msg::Stop => stop = true,
            }
            if !stop && jobs.len() < MAX_BATCH {
                next = rx.try_recv().ok();
            }
        }
        if !jobs.is_empty() {
            commit(&mut conn, jobs, shared, path);
            dirty = true;
            since_checkpoint += 1;
            if since_checkpoint >= BURST_CHECKPOINT {
                checkpoint(&conn, path, false, shared);
                since_checkpoint = 0;
            }
        }
        let seq = shared.get();
        for reply in flushes {
            let _ = reply.send(seq);
        }
        if stop {
            break;
        }
    }
    if let Err(e) = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())) {
        eprintln!(
            "store: {}: the closing checkpoint failed: {e}",
            path.display()
        );
    }
}

/// Runs a batch in one transaction and answers each operation.
fn commit(
    conn: &mut Connection,
    jobs: Vec<(Op, Option<SyncSender<Reply>>)>,
    shared: &Shared,
    path: &Path,
) {
    let count = jobs.len();
    shared.ops.fetch_add(count as u64, Ordering::Relaxed);
    shared.largest_batch.fetch_max(count, Ordering::Relaxed);
    let tx = match conn.transaction_with_behavior(TransactionBehavior::Immediate) {
        Ok(tx) => tx,
        Err(e) => {
            let why = format!("{}: the write could not begin: {e}", path.display());
            fail_all(jobs.into_iter().map(|(_, reply)| reply), &why, shared);
            return;
        }
    };
    let mut outcomes = Vec::with_capacity(count);
    for (op, reply) in jobs {
        outcomes.push((reply, run_one(&tx, op)));
    }
    let recorded = tx.execute(
        "UPDATE store_commit SET seq = seq + 1, at_ms = ?1 WHERE id = 1",
        [crate::now_ms()],
    );
    match recorded
        .map_err(StoreError::from)
        .and_then(|_| tx.commit().map_err(StoreError::from))
    {
        Ok(()) => {
            let seq = shared.advance();
            shared.commits.fetch_add(1, Ordering::Relaxed);
            for (reply, result) in outcomes {
                if result.is_err() {
                    shared.failed_ops.fetch_add(1, Ordering::Relaxed);
                }
                match reply {
                    Some(reply) => {
                        let _ = reply.send(result.map(|value| (value, seq)));
                    }
                    None => {
                        if let Err(e) = result {
                            eprintln!("store: {}: a write failed: {e}", path.display());
                        }
                    }
                }
            }
        }
        Err(e) => {
            let why = format!("{}: the commit failed: {e}", path.display());
            fail_all(outcomes.into_iter().map(|(reply, _)| reply), &why, shared);
        }
    }
}

fn fail_all(replies: impl Iterator<Item = Option<SyncSender<Reply>>>, why: &str, shared: &Shared) {
    eprintln!("store: {why}");
    for reply in replies {
        shared.failed_ops.fetch_add(1, Ordering::Relaxed);
        if let Some(reply) = reply {
            let _ = reply.send(Err(StoreError::invalid(why)));
        }
    }
}

/// Runs one operation inside its own savepoint: kept on success, rolled back on an error or a panic.
fn run_one(conn: &Connection, op: Op) -> Result<Value, StoreError> {
    conn.execute_batch("SAVEPOINT op")?;
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| op(conn)))
        .unwrap_or_else(|_| Err(StoreError::invalid("a write operation panicked")));
    let end = if result.is_ok() {
        conn.execute_batch("RELEASE op")
    } else {
        conn.execute_batch("ROLLBACK TO op; RELEASE op")
    };
    match end {
        Ok(()) => result,
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO op; RELEASE op");
            Err(e.into())
        }
    }
}

/// Checkpoints the WAL: passively in a burst, and when idle truncating it if it grew past its bound.
fn checkpoint(conn: &Connection, path: &Path, idle: bool, shared: &Shared) {
    let mode = if idle && wal_bytes(path) > WAL_TRUNCATE_BYTES {
        "TRUNCATE"
    } else {
        "PASSIVE"
    };
    match conn.query_row(&format!("PRAGMA wal_checkpoint({mode})"), [], |_| Ok(())) {
        Ok(()) => {
            shared.checkpoints.fetch_add(1, Ordering::Relaxed);
        }
        Err(e) => eprintln!("store: {}: checkpoint failed: {e}", path.display()),
    }
}

/// The size of the file's WAL now (0 when it has none).
pub(crate) fn wal_bytes(path: &Path) -> u64 {
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    std::fs::metadata(wal).map_or(0, |m| m.len())
}
