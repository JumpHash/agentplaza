//! Seat threads (PLAN 7.2, Engine 0.8): direct talk between sessions, addressed to seats.
//!
//! - A message goes to a seat (`T12/builder`, `T12/reviewer`, `planner`), never to a session. The hub delivers it to
//!   the session filling the seat now (a `message` work item); if none does, it waits on the seat and is the first
//!   thing the next session of that seat reads; if the task is done, it goes to the planner.
//! - A thread is a question and its answers; a task's journal links its threads, so a session that takes over a seat
//!   sees what was asked and settled.
//! - Loops are stopped: two seats exchanging more than [`LOOP_LIMIT`] messages in a thread without either closing it
//!   or producing an output is reported to the planner.
//!
//! The records live in memory; the hub's saver keeps them in `plaza.db` (tables `thread`, `thread_messages`).

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

/// Messages between two seats in one thread, with no output or close between them, past which it is a loop.
pub const LOOP_LIMIT: u32 = 8;

/// Where a thread is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Open,
    Closed,
}

/// One message of a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct Message {
    pub seq: u64,
    pub from_seat: String,
    pub from_session: Option<String>,
    pub to_seat: String,
    #[table(content)]
    pub text: String,
    pub at_ms: u64,
    /// The session it was given to, once given.
    pub delivered_session: Option<String>,
    pub delivered_ms: Option<u64>,
}

/// One thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "thread")]
pub struct Thread {
    /// `th<seq>`.
    #[table(key)]
    pub id: String,
    #[table(index)]
    pub task: String,
    pub subject: Option<String>,
    pub state: State,
    pub opened_by: String,
    pub next_seq: u64,
    /// Messages since the last output or reset: the loop stop counts these.
    pub since_output: u32,
    /// The loop was reported to the planner.
    pub reported: bool,
    pub created_ms: u64,
    pub closed_ms: Option<u64>,
    #[table(child)]
    pub messages: Vec<Message>,
}

/// What sending a message did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    pub thread: String,
    pub message: Message,
    /// The exchange just passed [`LOOP_LIMIT`]: report it to the planner (once per thread).
    pub looped: bool,
}

#[derive(Default)]
struct Inner {
    next: u64,
    threads: BTreeMap<String, Thread>,
}

/// Every thread.
#[derive(Default)]
pub struct Threads {
    inner: Mutex<Inner>,
}

/// A seat address: `<task>/<seat>` or `planner` (the project's planner).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Address {
    Seat { task: String, seat: String },
    Planner,
}

/// Reads a seat address as an agent writes it (`T12/builder`, `t12/reviewer`, `planner`). `task` is the sender's
/// task: a bare seat name (`reviewer`) means a seat of the sender's own task.
pub fn address(to: &str, task: Option<&str>) -> Option<Address> {
    let to = to.trim();
    if to.eq_ignore_ascii_case("planner") {
        return Some(Address::Planner);
    }
    match to.split_once('/') {
        Some((t, seat)) if !t.is_empty() && !seat.is_empty() => Some(Address::Seat {
            task: t.to_string(),
            seat: seat.to_string(),
        }),
        None if !to.is_empty() => task.map(|t| Address::Seat {
            task: t.to_string(),
            seat: to.to_string(),
        }),
        _ => None,
    }
}

impl Threads {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sends `text` from `from_seat` to `to_seat` in `task`: in `thread` when given (it must belong to the task), else
    /// in a new thread.
    #[allow(clippy::too_many_arguments)]
    pub fn send(
        &self,
        task: &str,
        thread: Option<&str>,
        from_seat: &str,
        from_session: Option<&str>,
        to_seat: &str,
        text: &str,
        subject: Option<&str>,
        now_ms: u64,
    ) -> Result<Sent, String> {
        let mut inner = self.lock();
        let id = match thread {
            Some(id) => {
                let t = inner
                    .threads
                    .get(id)
                    .ok_or_else(|| format!("no thread {id}"))?;
                if t.task != task {
                    return Err(format!("thread {id} belongs to task {}", t.task));
                }
                id.to_string()
            }
            None => {
                inner.next += 1;
                let id = format!("th{}", inner.next);
                inner.threads.insert(
                    id.clone(),
                    Thread {
                        id: id.clone(),
                        task: task.to_string(),
                        subject: subject.map(str::to_string),
                        state: State::Open,
                        opened_by: from_seat.to_string(),
                        next_seq: 1,
                        since_output: 0,
                        reported: false,
                        created_ms: now_ms,
                        closed_ms: None,
                        messages: Vec::new(),
                    },
                );
                id
            }
        };
        let t = inner.threads.get_mut(&id).expect("present");
        t.state = State::Open;
        let message = Message {
            seq: t.next_seq,
            from_seat: from_seat.to_string(),
            from_session: from_session.map(str::to_string),
            to_seat: to_seat.to_string(),
            text: text.to_string(),
            at_ms: now_ms,
            delivered_session: None,
            delivered_ms: None,
        };
        t.next_seq += 1;
        t.since_output += 1;
        t.messages.push(message.clone());
        let looped = t.since_output > LOOP_LIMIT && !t.reported;
        if looped {
            t.reported = true;
        }
        Ok(Sent {
            thread: id,
            message,
            looped,
        })
    }

    /// The messages waiting on `task`'s seat `seat` (not yet given to a session), oldest first, with their threads.
    pub fn waiting(&self, task: &str, seat: &str) -> Vec<(String, Message)> {
        let inner = self.lock();
        let mut out: Vec<(String, Message)> = inner
            .threads
            .values()
            .filter(|t| t.task == task)
            .flat_map(|t| {
                t.messages
                    .iter()
                    .filter(|m| m.to_seat == seat && m.delivered_session.is_none())
                    .map(move |m| (t.id.clone(), m.clone()))
            })
            .collect();
        out.sort_by_key(|(_, m)| m.at_ms);
        out
    }

    /// Records that message `seq` of `thread` was given to `session`.
    pub fn delivered(&self, thread: &str, seq: u64, session: &str, now_ms: u64) {
        let mut inner = self.lock();
        if let Some(m) = inner
            .threads
            .get_mut(thread)
            .and_then(|t| t.messages.iter_mut().find(|m| m.seq == seq))
        {
            m.delivered_session = Some(session.to_string());
            m.delivered_ms = Some(now_ms);
        }
    }

    /// A seat of `task` produced an output (a step ended): its threads' loop counts start again.
    pub fn output(&self, task: &str) {
        let mut inner = self.lock();
        for t in inner.threads.values_mut().filter(|t| t.task == task) {
            t.since_output = 0;
        }
    }

    /// Closes a thread (the question is settled).
    pub fn close(&self, thread: &str, now_ms: u64) -> Option<Thread> {
        let mut inner = self.lock();
        let t = inner.threads.get_mut(thread)?;
        t.state = State::Closed;
        t.closed_ms = Some(now_ms);
        t.since_output = 0;
        Some(t.clone())
    }

    pub fn get(&self, thread: &str) -> Option<Thread> {
        self.lock().threads.get(thread).cloned()
    }

    /// The threads of `task`, oldest first.
    pub fn of_task(&self, task: &str) -> Vec<Thread> {
        self.lock()
            .threads
            .values()
            .filter(|t| t.task == task)
            .cloned()
            .collect()
    }

    /// The threads' rows, for the hub's store.
    pub fn rows(&self) -> Result<apz_store::RowSet, apz_store::StoreError> {
        let inner = self.lock();
        apz_store::RowSet::list(def(), inner.threads.values())
    }

    /// Replaces the threads with what the store holds (at start).
    pub fn load(
        &self,
        conn: &apz_store::rusqlite::Connection,
    ) -> Result<(), apz_store::StoreError> {
        let stored: Vec<Thread> = apz_store::table::load_list(conn, def())?;
        let mut inner = Inner::default();
        for t in stored {
            if let Some(n) = t.id.strip_prefix("th").and_then(|n| n.parse::<u64>().ok()) {
                inner.next = inner.next.max(n);
            }
            inner.threads.insert(t.id.clone(), t);
        }
        *self.lock() = inner;
        Ok(())
    }
}

/// The threads' tables.
pub fn def() -> &'static apz_store::TableDef {
    apz_store::root_def!(Thread)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_read_as_agents_write_them() {
        assert_eq!(address("planner", None), Some(Address::Planner));
        assert_eq!(
            address("t12/builder", None),
            Some(Address::Seat {
                task: "t12".into(),
                seat: "builder".into()
            })
        );
        assert_eq!(
            address("reviewer", Some("t3")),
            Some(Address::Seat {
                task: "t3".into(),
                seat: "reviewer".into()
            })
        );
        assert_eq!(address("reviewer", None), None);
        assert_eq!(address("/x", None), None);
    }

    #[test]
    fn a_message_to_an_empty_seat_waits_for_its_next_session() {
        let threads = Threads::default();
        let sent = threads
            .send(
                "t1",
                None,
                "reviewer",
                Some("s2"),
                "builder",
                "which JSON shape?",
                Some("shape"),
                1,
            )
            .unwrap();
        assert_eq!(threads.waiting("t1", "builder").len(), 1);
        threads.delivered(&sent.thread, sent.message.seq, "s3", 2);
        assert!(threads.waiting("t1", "builder").is_empty());
        threads
            .send(
                "t1",
                Some(&sent.thread),
                "builder",
                Some("s3"),
                "reviewer",
                "{rows: [...]}",
                None,
                3,
            )
            .unwrap();
        assert_eq!(threads.get(&sent.thread).unwrap().messages.len(), 2);
        assert!(
            threads
                .send("t2", Some(&sent.thread), "x", None, "y", "z", None, 4)
                .is_err(),
            "not another task's thread"
        );
    }

    #[test]
    fn a_ping_pong_is_reported_once_and_an_output_resets_it() {
        let threads = Threads::default();
        let first = threads
            .send("t1", None, "a", None, "b", "1", None, 1)
            .unwrap();
        let mut looped = 0;
        for i in 0..LOOP_LIMIT + 3 {
            let (from, to) = if i % 2 == 0 { ("b", "a") } else { ("a", "b") };
            let sent = threads
                .send("t1", Some(&first.thread), from, None, to, "again", None, 2)
                .unwrap();
            looped += usize::from(sent.looped);
        }
        assert_eq!(looped, 1, "reported once");
        threads.output("t1");
        assert_eq!(threads.get(&first.thread).unwrap().since_output, 0);
    }

    #[test]
    fn threads_round_trip() {
        let threads = Threads::default();
        threads
            .send("t1", None, "a", None, "b", "hi", Some("s"), 1)
            .unwrap();
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &threads.rows().unwrap()).unwrap();
        let loaded = Threads::default();
        loaded.load(&conn).unwrap();
        assert_eq!(loaded.of_task("t1"), threads.of_task("t1"));
    }
}
