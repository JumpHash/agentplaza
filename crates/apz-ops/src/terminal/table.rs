//! The terminal table the platforms share: the ring of output a terminal keeps, the table of terminals with
//! its idle janitor, and the rules for the arguments of each call. A platform backend opens the shell on its
//! pseudo console and ends it; nothing here knows how a terminal is made.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::error::OpError;

/// Terminals open at once, the exited ones included until they are closed.
pub(crate) const MAX_TERMS: usize = 4;
/// How long a terminal may go without a read, write or resize before it is closed.
pub(crate) const IDLE: Duration = Duration::from_secs(30 * 60);
/// How often the janitor looks for idle terminals when no call does.
pub(crate) const JANITOR_EVERY: Duration = Duration::from_secs(60);
/// The most output a terminal keeps; older bytes are dropped.
pub(crate) const RING_CAP: usize = 1 << 20;
/// The most a read returns, in raw bytes. Base64 makes it 4/3 of that, so 48 KiB would not fit the
/// operator's output cap with the reply around it; 47 KiB does.
pub(crate) const READ_MAX: usize = 47 * 1024;
/// The most a write takes, in raw bytes.
pub(crate) const WRITE_MAX: usize = 64 * 1024;
/// The largest number of columns or rows.
pub(crate) const MAX_SIZE: u64 = 1000;

/// The terminals one table holds, by id.
pub(crate) struct Table<T> {
    pub(crate) next: u64,
    pub(crate) terms: BTreeMap<u64, T>,
}

impl<T> Default for Table<T> {
    fn default() -> Self {
        Self {
            next: 0,
            terms: BTreeMap::new(),
        }
    }
}

/// What the table needs from a terminal: when it was last used, and a way to note a use.
pub(crate) trait Idle {
    fn last_active(&self) -> Instant;
    fn touch(&mut self);
}

/// The last [`RING_CAP`] bytes of output, and how many bytes have ever been written.
#[derive(Default)]
pub(crate) struct Ring {
    bytes: VecDeque<u8>,
    total: u64,
}

/// One read from a ring: the bytes, how many bytes were lost before `offset`, the offset the bytes start at, and
/// the offset to read from next.
pub(crate) struct Slice {
    pub(crate) data: Vec<u8>,
    pub(crate) lost: u64,
    pub(crate) offset: u64,
    pub(crate) next: u64,
}

impl Ring {
    /// Appends output, dropping the oldest bytes past [`RING_CAP`].
    pub(crate) fn append(&mut self, data: &[u8]) {
        self.total += data.len() as u64;
        self.bytes.extend(data);
        if self.bytes.len() > RING_CAP {
            let excess = self.bytes.len() - RING_CAP;
            self.bytes.drain(..excess);
        }
    }

    /// The offset of the oldest byte kept.
    fn oldest(&self) -> u64 {
        self.total - self.bytes.len() as u64
    }

    /// The output from `offset`, at most [`READ_MAX`] bytes. An offset the ring has dropped moves on to the
    /// oldest byte kept, and the slice says how many bytes were lost.
    pub(crate) fn read(&self, offset: u64) -> Result<Slice, OpError> {
        let total = self.total;
        if offset > total {
            return Err(OpError::invalid(format!(
                "offset {offset} is past the end of the output ({total} bytes)"
            )));
        }
        let oldest = self.oldest();
        let lost = oldest.saturating_sub(offset);
        let start = offset.max(oldest);
        let len = (total - start).min(READ_MAX as u64) as usize;
        let from = (start - oldest) as usize;
        let data: Vec<u8> = self.bytes.range(from..from + len).copied().collect();
        Ok(Slice {
            data,
            lost,
            offset: start,
            next: start + len as u64,
        })
    }
}

/// The bytes a write sends: base64 `data_b64`, decoded, at most [`WRITE_MAX`].
pub(crate) fn decode_write(data_b64: &str) -> Result<Vec<u8>, OpError> {
    let data = STANDARD
        .decode(data_b64)
        .map_err(|err| OpError::invalid(format!("\"data_b64\" is not base64: {err}")))?;
    if data.len() > WRITE_MAX {
        return Err(OpError::limit(format!(
            "{} bytes is over the {WRITE_MAX}-byte write cap",
            data.len()
        )));
    }
    Ok(data)
}

/// Base64 of `data`, as a read reply carries it.
pub(crate) fn encode_read(data: &[u8]) -> String {
    STANDARD.encode(data)
}

/// Starts the janitor: a thread that closes idle terminals every [`JANITOR_EVERY`], until the table is dropped.
pub(crate) fn spawn_janitor<T: Idle + Send + 'static>(table: &Arc<Mutex<Table<T>>>, retire: fn(T)) {
    let watch = Arc::downgrade(table);
    thread::spawn(move || {
        loop {
            thread::sleep(JANITOR_EVERY);
            match watch.upgrade() {
                Some(table) => sweep(&table, retire),
                None => break,
            }
        }
    });
}

/// Runs `act` on the terminal `name` names, and counts the call as activity on it.
pub(crate) fn with_term<T: Idle, R>(
    table: &Mutex<Table<T>>,
    name: &str,
    act: impl FnOnce(&mut T) -> R,
) -> Result<R, OpError> {
    let id = parse_term(name)?;
    let mut table = locked(table);
    let term = table
        .terms
        .get_mut(&id)
        .ok_or_else(|| OpError::not_found(format!("no terminal {name:?}")))?;
    term.touch();
    Ok(act(term))
}

/// Removes the terminal `name` names from the table and returns it, for its owner to end.
pub(crate) fn remove_term<T>(table: &Mutex<Table<T>>, name: &str) -> Result<T, OpError> {
    let id = parse_term(name)?;
    locked(table)
        .terms
        .remove(&id)
        .ok_or_else(|| OpError::not_found(format!("no terminal {name:?}")))
}

/// Closes, in the background, every terminal idle for [`IDLE`]. `retire` ends one.
pub(crate) fn sweep<T: Idle + Send + 'static>(table: &Mutex<Table<T>>, retire: fn(T)) {
    for term in take_idle(table, Instant::now()) {
        thread::spawn(move || retire(term));
    }
}

/// Removes and returns the terminals idle at `now`.
pub(crate) fn take_idle<T: Idle>(table: &Mutex<Table<T>>, now: Instant) -> Vec<T> {
    let mut table = locked(table);
    let idle: Vec<u64> = table
        .terms
        .iter()
        .filter(|(_, term)| now.duration_since(term.last_active()) >= IDLE)
        .map(|(id, _)| *id)
        .collect();
    idle.into_iter()
        .filter_map(|id| table.terms.remove(&id))
        .collect()
}

/// A size argument: a whole number from 1 to [`MAX_SIZE`].
pub(crate) fn size(value: u64, key: &str) -> Result<u16, OpError> {
    if (1..=MAX_SIZE).contains(&value) {
        Ok(value as u16)
    } else {
        Err(OpError::invalid(format!(
            "{key:?} must be a whole number from 1 to {MAX_SIZE}"
        )))
    }
}

pub(crate) fn term_name(id: u64) -> String {
    format!("term-{id}")
}

/// The id a terminal name stands for. Only the canonical spelling is accepted, so one terminal has one
/// name.
pub(crate) fn parse_term(name: &str) -> Result<u64, OpError> {
    name.strip_prefix("term-")
        .and_then(|digits| digits.parse::<u64>().ok())
        .filter(|id| term_name(*id) == name)
        .ok_or_else(|| OpError::invalid(format!("{name:?} is not a terminal name")))
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

pub(crate) fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
