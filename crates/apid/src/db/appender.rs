//! The transcript writer. `Db::append_transcript` only queues a record here; this thread, with a connection of its
//! own, writes the queued records in batches, one transaction per batch, so an agent's decision does not wait for
//! Postgres. A record the database cannot take now goes to the spool in `db` (held in order, and written again when
//! the database answers); a record Postgres refuses is dropped and logged, as an append always was.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use super::{
    APPEND_DRAIN, PendingRow, SPOOL_CAP, Spool, connect_raw, database_unavailable, due, sql,
};

/// How long the writer waits for the rows after the first of a batch (a batch goes at once when it is full).
const BATCH_WAIT: Duration = Duration::from_millis(20);
/// The records the queue holds; `append_transcript` spools a record rather than wait when it is full.
const QUEUE: usize = 10_000;
/// The wait before the first reconnect; each failure doubles it, up to `BACKOFF_MAX`.
const BACKOFF_FIRST: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// How long `Db` waits for the writer to answer a flush or a close.
pub(super) const ANSWER_WAIT: Duration = Duration::from_secs(5);

/// What the writer takes from its queue.
#[derive(Debug)]
pub(super) enum Msg {
    /// A transcript record to write.
    Row(PendingRow),
    /// Answered once the records sent before it are written or held, and one pass of the spool has run.
    Flush(SyncSender<()>),
    /// Writes what it holds (the spool too, as far as the database answers), answers, and stops.
    Close(SyncSender<()>),
}

/// Starts the writer for the database at `url`, sharing the spool and the log throttle with `Db`. Returns the queue
/// the records are sent on, or `None` when the thread cannot start (the caller then holds the records in the spool).
pub(super) fn spawn(
    url: String,
    spool: Arc<Mutex<Spool>>,
    dropped: Arc<AtomicU64>,
    logged: Arc<Mutex<Option<Instant>>>,
) -> Option<SyncSender<Msg>> {
    let (tx, rx) = sync_channel(QUEUE);
    let writer = Writer {
        url,
        client: None,
        retry_at: None,
        backoff: BACKOFF_FIRST,
        spool,
        dropped,
        logged,
    };
    match std::thread::Builder::new()
        .name("db-appender".into())
        .spawn(move || run(writer, rx))
    {
        Ok(_) => Some(tx),
        Err(e) => {
            eprintln!(
                "db: transcript writer not started: {e}; transcript records are held in memory"
            );
            None
        }
    }
}

/// The writer's loop: takes a batch off the queue, writes it, and answers the control message that ended it. Returns
/// when the queue is closed, or after a `Close`, once it has written what it holds.
fn run(mut writer: Writer, rx: Receiver<Msg>) {
    while let Some((rows, control)) = collect_batch(&rx, APPEND_DRAIN, BATCH_WAIT) {
        if !rows.is_empty() {
            writer.write(rows);
        }
        match control {
            Some(Msg::Flush(reply)) => {
                writer.drain_once();
                let _ = reply.send(());
            }
            Some(Msg::Close(reply)) => {
                writer.drain_all();
                let _ = reply.send(());
                return;
            }
            _ => {}
        }
    }
    writer.drain_all();
}

/// The next batch: the rows from the first queued message on, at most `max` of them, taken within `wait` of the
/// first (rows already queued are taken without waiting). A control message ends the batch early and comes back with
/// it. `None` once the queue is closed and empty.
fn collect_batch(
    rx: &Receiver<Msg>,
    max: usize,
    wait: Duration,
) -> Option<(Vec<PendingRow>, Option<Msg>)> {
    let deadline = Instant::now() + wait;
    let mut rows = Vec::new();
    let mut msg = rx.recv().ok()?;
    loop {
        match msg {
            Msg::Row(row) => rows.push(row),
            control => return Some((rows, Some(control))),
        }
        if rows.len() >= max {
            return Some((rows, None));
        }
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(next) => msg = next,
            Err(_) => return Some((rows, None)),
        }
    }
}

/// The writer's state: its connection (opened when it is needed, and reopened after a failure, with a backoff between
/// attempts), and the spool and log throttle it shares with `Db`.
struct Writer {
    url: String,
    client: Option<postgres::Client>,
    /// No connection attempt before this instant.
    retry_at: Option<Instant>,
    backoff: Duration,
    spool: Arc<Mutex<Spool>>,
    dropped: Arc<AtomicU64>,
    logged: Arc<Mutex<Option<Instant>>>,
}

impl Writer {
    /// The connection, opened if it is not open and the backoff allows an attempt. `None` while the database cannot
    /// be reached.
    fn connection(&mut self) -> Option<&mut postgres::Client> {
        if self.client.is_none() {
            if self.retry_at.is_some_and(|at| Instant::now() < at) {
                return None;
            }
            match connect_raw(&self.url) {
                Ok(client) => {
                    self.client = Some(client);
                    self.backoff = BACKOFF_FIRST;
                }
                Err(e) => {
                    self.unavailable(&e.to_string());
                    return None;
                }
            }
        }
        self.client.as_mut()
    }

    /// Drops the connection after a failure: the next attempt waits the backoff, which doubles up to `BACKOFF_MAX`.
    fn unavailable(&mut self, reason: &str) {
        self.client = None;
        self.retry_at = Some(Instant::now() + self.backoff);
        self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
        log_once(
            &self.logged,
            format!("db: the transcript writer cannot write to the database ({reason}); retrying"),
        );
    }

    /// Writes a batch of new records. The ones the database cannot take are held in the spool, behind the records
    /// already there. A batch that goes in is followed by one pass of the spool.
    fn write(&mut self, mut rows: Vec<PendingRow>) {
        let logged = Arc::clone(&self.logged);
        let Some(client) = self.connection() else {
            self.hold(rows, "the database is unreachable");
            return;
        };
        match write_or_return(client, &rows, &logged) {
            Ok(()) => {
                self.drain_once();
            }
            Err((written, reason)) => {
                let held = rows.split_off(written);
                self.unavailable(&reason);
                self.hold(held, &reason);
            }
        }
    }

    /// Holds `rows` in the spool, behind the records already held.
    fn hold(&self, rows: Vec<PendingRow>, reason: &str) {
        let mut spool = self.spool.lock().unwrap_or_else(PoisonError::into_inner);
        for row in rows {
            hold_row(&mut spool, &self.dropped, &self.logged, row, reason);
        }
    }

    /// One pass of the spool: its oldest `APPEND_DRAIN` records go in one transaction. The spool stays locked while
    /// they are written, which only a full spool's append waits for. Returns how many records left the spool.
    fn drain_once(&mut self) -> usize {
        let (shared, logged) = (Arc::clone(&self.spool), Arc::clone(&self.logged));
        let Some(client) = self.connection() else {
            return 0;
        };
        let mut spool = shared.lock().unwrap_or_else(PoisonError::into_inner);
        let count = spool.rows.len().min(APPEND_DRAIN);
        if count == 0 {
            return 0;
        }
        let left = match write_or_return(client, &spool.rows.make_contiguous()[..count], &logged) {
            Ok(()) => count,
            Err((written, reason)) => {
                self.unavailable(&reason);
                written
            }
        };
        let removed = spool.drain_while(left, |_| true);
        if removed > 0 && spool.rows.is_empty() {
            eprintln!("db: the database answers again: the held transcript records are written");
        }
        removed
    }

    /// Writes the spool until it is empty, or until the database stops answering.
    fn drain_all(&mut self) {
        while self.drain_once() > 0 {}
    }
}

/// Writes `rows` in order: one transaction for the lot. When Postgres refuses one of them, each row is written on its
/// own, so only the refused rows are dropped (logged). `Err((written, reason))` when the database is unavailable: the
/// first `written` rows are in, the rest are not.
fn write_or_return(
    client: &mut postgres::Client,
    rows: &[PendingRow],
    logged: &Mutex<Option<Instant>>,
) -> Result<(), (usize, String)> {
    match write_rows(client, rows) {
        Ok(()) => return Ok(()),
        Err(e) if database_unavailable(&e) => return Err((0, e.to_string())),
        Err(_) => {}
    }
    for (index, row) in rows.iter().enumerate() {
        match write_rows(client, std::slice::from_ref(row)) {
            Ok(()) => {}
            Err(e) if database_unavailable(&e) => return Err((index, e.to_string())),
            Err(e) => log_once(
                logged,
                format!("db: held transcript record refused: {e} (dropped; logged once a minute)"),
            ),
        }
    }
    Ok(())
}

/// Writes `rows` in one transaction, the statement prepared once (a record whose `(cid, seq)` is stored is ignored).
fn write_rows(client: &mut postgres::Client, rows: &[PendingRow]) -> Result<(), postgres::Error> {
    let mut tx = client.transaction()?;
    let statement = tx.prepare(sql::TRANSCRIPT_APPEND)?;
    for row in rows {
        let (cid, seq, work, at_ms) = (
            row.cid as i64,
            row.seq as i64,
            row.work as i64,
            row.at_ms as i64,
        );
        tx.execute(
            &statement,
            &[&cid, &seq, &work, &row.kind, &at_ms, &row.data],
        )?;
    }
    tx.commit()
}

/// Holds `row` in the spool, behind the records already held. A full spool drops its oldest record, counted in
/// `dropped` and logged once a minute.
pub(super) fn hold_row(
    spool: &mut Spool,
    dropped: &AtomicU64,
    logged: &Mutex<Option<Instant>>,
    row: PendingRow,
    reason: &str,
) {
    if spool.rows.is_empty() {
        eprintln!(
            "db: transcript append failed: {reason}; holding transcript records in memory until the database answers (up to {SPOOL_CAP}, then the oldest is dropped)"
        );
    }
    if spool.push(row) {
        dropped.fetch_add(1, Ordering::Relaxed);
        log_once(
            logged,
            format!(
                "db: transcript spool full ({SPOOL_CAP} records): the oldest record is dropped (logged once a minute)"
            ),
        );
    }
}

/// Prints `message` unless a transcript message was printed in the last minute.
pub(super) fn log_once(logged: &Mutex<Option<Instant>>, message: String) {
    let mut last = logged.lock().unwrap_or_else(PoisonError::into_inner);
    if due(*last) {
        *last = Some(Instant::now());
        eprintln!("{message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Db, test_database};
    use std::collections::BTreeMap;

    fn row(seq: u64) -> PendingRow {
        PendingRow {
            cid: 7,
            seq,
            work: 1,
            kind: "tool".to_string(),
            at_ms: seq,
            data: serde_json::Value::Null,
        }
    }

    fn seqs(rows: &[PendingRow]) -> Vec<u64> {
        rows.iter().map(|r| r.seq).collect()
    }

    /// A batch takes the queued rows up to its cap; the rest come in the next batch; a closed, empty queue ends them.
    #[test]
    fn a_batch_takes_the_queued_rows_up_to_its_cap() {
        let (tx, rx) = sync_channel(10);
        for seq in 1..=5 {
            tx.send(Msg::Row(row(seq))).unwrap();
        }
        drop(tx);
        let (first, control) = collect_batch(&rx, 3, BATCH_WAIT).expect("a batch");
        assert_eq!(seqs(&first), vec![1, 2, 3]);
        assert!(control.is_none());
        let (rest, _) = collect_batch(&rx, 3, BATCH_WAIT).expect("the rest");
        assert_eq!(seqs(&rest), vec![4, 5]);
        assert!(collect_batch(&rx, 3, BATCH_WAIT).is_none());
    }

    /// A control message ends the batch early, and comes back with the rows before it.
    #[test]
    fn a_flush_ends_the_batch_and_comes_back_with_it() {
        let (tx, rx) = sync_channel(10);
        tx.send(Msg::Row(row(1))).unwrap();
        let (reply, _answer) = sync_channel(1);
        tx.send(Msg::Flush(reply)).unwrap();
        tx.send(Msg::Row(row(2))).unwrap();
        let (rows, control) = collect_batch(&rx, 500, BATCH_WAIT).expect("a batch");
        assert_eq!(seqs(&rows), vec![1]);
        assert!(matches!(control, Some(Msg::Flush(_))));
    }

    /// 2 000 records appended quickly are all written by the flush, and read back in seq order.
    #[test]
    fn two_thousand_appended_records_are_read_back_in_seq_order() {
        let Some(url) = test_database("appender") else {
            return;
        };
        let env = BTreeMap::from([("APZ_DATABASE_URL".to_string(), url)]);
        let db = Db::connect(&env);
        assert!(db.enabled());
        let cid = 8_000_000_000_000_000_000u64 + u64::from(std::process::id());
        let data = serde_json::json!({ "n": 1 });
        for seq in 1..=2_000u64 {
            db.append_transcript(cid, seq, 1, "test", seq, &data);
        }
        db.flush_transcripts();
        let read: Vec<u64> = db.transcript(cid, 5_000).iter().map(|r| r.0).collect();
        assert_eq!(read, (1..=2_000).collect::<Vec<u64>>());
        db.delete_transcript(cid);
    }
}
