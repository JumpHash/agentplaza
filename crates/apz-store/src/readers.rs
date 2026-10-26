//! A file's reader connections: a pool sized to the CPUs, each connection used by one thread at a time.

use std::ops::Deref;
use std::path::PathBuf;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use rusqlite::Connection;

use crate::conn::{self, Role};
use crate::{Durability, StoreError};

/// How long a reader waits for a free connection before it reports the pool exhausted. Reads are short; a pool this
/// busy for this long means something holds connections it should have returned.
const CHECKOUT_TIMEOUT: Duration = Duration::from_secs(30);

/// The pool of a file's reader connections. Connections open on first use, up to `max`; a caller past that waits for
/// one to come back.
pub struct Readers {
    path: PathBuf,
    durability: Durability,
    max: usize,
    state: Mutex<PoolState>,
    returned: Condvar,
}

struct PoolState {
    idle: Vec<Connection>,
    open: usize,
    waiting: usize,
}

impl Readers {
    pub(crate) fn new(path: PathBuf, durability: Durability, max: usize) -> Readers {
        Readers {
            path,
            durability,
            max: max.max(1),
            state: Mutex::new(PoolState {
                idle: Vec::new(),
                open: 0,
                waiting: 0,
            }),
            returned: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, PoolState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A connection of the pool, back in it when the [`Reader`] drops.
    pub fn get(&self) -> Result<Reader<'_>, StoreError> {
        let mut state = self.lock();
        loop {
            if let Some(conn) = state.idle.pop() {
                return Ok(Reader {
                    conn: Some(conn),
                    pool: self,
                });
            }
            if state.open < self.max {
                state.open += 1;
                drop(state);
                return match conn::open(&self.path, self.durability, Role::Reader) {
                    Ok(conn) => Ok(Reader {
                        conn: Some(conn),
                        pool: self,
                    }),
                    Err(e) => {
                        self.lock().open -= 1;
                        self.returned.notify_one();
                        Err(e)
                    }
                };
            }
            state.waiting += 1;
            let (next, timeout) = self
                .returned
                .wait_timeout(state, CHECKOUT_TIMEOUT)
                .unwrap_or_else(PoisonError::into_inner);
            state = next;
            state.waiting -= 1;
            if timeout.timed_out() && state.idle.is_empty() && state.open >= self.max {
                return Err(StoreError::invalid(format!(
                    "{}: every reader connection has been busy for {}s",
                    self.path.display(),
                    CHECKOUT_TIMEOUT.as_secs()
                )));
            }
        }
    }

    /// Runs `f` on a connection inside one read transaction, so every statement in it sees the same snapshot.
    pub fn read<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let reader = self.get()?;
        reader.prepare_cached("BEGIN DEFERRED")?.execute([])?;
        let value = f(&reader)?;
        // A read transaction has nothing to commit; ending it releases its snapshot.
        reader.prepare_cached("COMMIT")?.execute([])?;
        Ok(value)
    }

    /// How many connections are open now (idle or in use).
    pub fn open_count(&self) -> usize {
        self.lock().open
    }

    /// Closes the idle connections (the store is closing).
    pub(crate) fn close_idle(&self) {
        let mut state = self.lock();
        let idle = std::mem::take(&mut state.idle);
        state.open -= idle.len();
        drop(state);
        drop(idle);
    }
}

/// A reader connection checked out of its pool.
pub struct Reader<'a> {
    conn: Option<Connection>,
    pool: &'a Readers,
}

impl Deref for Reader<'_> {
    type Target = Connection;

    fn deref(&self) -> &Connection {
        self.conn
            .as_ref()
            .expect("a reader holds its connection until it drops")
    }
}

impl Drop for Reader<'_> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            // A connection left inside a transaction (a caller's error path) is rolled back before it is reused.
            if !conn.is_autocommit() {
                let _ = conn.execute_batch("ROLLBACK");
            }
            let mut state = self.pool.lock();
            state.idle.push(conn);
            // Read under the lock `get` waits with, so no waiter can miss this wakeup.
            let waiting = state.waiting > 0;
            drop(state);
            if waiting {
                self.pool.returned.notify_one();
            }
        }
    }
}
