//! The run table the platforms share: the output tail a run keeps, the registry of runs, and the rules for a
//! start, a poll and a stop. A platform backend starts the process, supervises it and reports into its [`Run`];
//! nothing here knows how a process is started or ended.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use super::{DEFAULT_TIMEOUT_S, MAX_RUNS, MAX_TIMEOUT_S, OUTPUT_CAP, RunPoll, RunStopped};
use crate::error::OpError;
use crate::page::{OUTPUT_BUDGET, byte_chunk_end, is_continuation};

/// How long a finished run stays to be polled.
pub(crate) const RUN_KEEP: Duration = Duration::from_secs(10 * 60);
/// How often a waiting loop looks again.
pub(crate) const RUN_POLL: Duration = Duration::from_millis(20);
/// How long the output is awaited once the shell and its tree are gone.
pub(crate) const RUN_DRAIN: Duration = Duration::from_millis(500);
/// How long a stop waits for the run to end before it answers.
const RUN_STOP_WAIT: Duration = Duration::from_secs(2);

/// The runs one table holds.
#[derive(Default)]
pub(crate) struct Registry {
    next: u64,
    runs: BTreeMap<String, Arc<Run>>,
}

/// One run: its output tail, how it ended, and whether a stop was asked for.
#[derive(Default)]
pub(crate) struct Run {
    pub(crate) output: Mutex<Tail>,
    pub(crate) state: Mutex<RunState>,
    pub(crate) stop: AtomicBool,
}

/// The last [`OUTPUT_CAP`] bytes of a run's output, and the offset of the first.
#[derive(Default)]
pub(crate) struct Tail {
    bytes: Vec<u8>,
    base: u64,
}

#[derive(Default)]
pub(crate) enum RunState {
    #[default]
    Running,
    Done {
        exit_code: Option<i32>,
        timed_out: bool,
        at: Instant,
    },
}

/// The timeout a start asks for, in seconds: the default when none is given, and refused when it is zero or
/// over the cap. A command that is empty is refused first.
pub(crate) fn timeout_secs(command: &str, timeout_s: Option<u64>) -> Result<u64, OpError> {
    if command.trim().is_empty() {
        return Err(OpError::invalid("\"command\" must be a non-empty string"));
    }
    match timeout_s {
        None => Ok(DEFAULT_TIMEOUT_S),
        Some(0) => Err(OpError::invalid("\"timeout_s\" must be at least 1")),
        Some(secs) if secs > MAX_TIMEOUT_S => Err(OpError::limit(format!(
            "\"timeout_s\" is {secs}, over the {MAX_TIMEOUT_S}-second cap"
        ))),
        Some(secs) => Ok(secs),
    }
}

impl Registry {
    /// A registry with no runs.
    pub(crate) const fn new() -> Self {
        Self {
            next: 0,
            runs: BTreeMap::new(),
        }
    }

    /// Makes room for one more run. A finished run older than [`RUN_KEEP`] is dropped; when [`MAX_RUNS`] runs are
    /// still held, the oldest finished one goes. When all are running the start is refused.
    pub(crate) fn admit(&mut self, now: Instant) -> Result<(), OpError> {
        self.runs.retain(|_, run| !run.expired(now));
        if self.runs.len() < MAX_RUNS {
            return Ok(());
        }
        let oldest_finished = self
            .runs
            .iter()
            .filter_map(|(id, run)| run.finished_at().map(|at| (at, id.clone())))
            .min();
        match oldest_finished {
            Some((_, id)) => {
                self.runs.remove(&id);
                Ok(())
            }
            None => Err(OpError::limit(format!(
                "{MAX_RUNS} runs are running; stop one first"
            ))),
        }
    }

    /// Records a started run and returns its id.
    pub(crate) fn insert(&mut self, run: Arc<Run>) -> String {
        self.next += 1;
        let id = format!("run-{}", self.next);
        self.runs.insert(id.clone(), run);
        id
    }
}

/// The run `id` names.
pub(crate) fn lookup(registry: &Mutex<Registry>, id: &str) -> Result<Arc<Run>, OpError> {
    locked(registry)
        .runs
        .get(id)
        .cloned()
        .ok_or_else(|| OpError::not_found(format!("no run {id}")))
}

/// The run's output from `offset` (the start when `None`), at most one chunk.
///
/// `offset` is where the output starts, which is later than asked when the tail has dropped earlier bytes;
/// `next` is the offset to poll from next.
pub(crate) fn poll_run(
    registry: &Mutex<Registry>,
    run: &str,
    offset: Option<u64>,
) -> Result<RunPoll, OpError> {
    let found = lookup(registry, run)?;
    let offset = offset.unwrap_or(0);
    let (done, exit_code, timed_out) = match *locked(&found.state) {
        RunState::Running => (false, None, false),
        RunState::Done {
            exit_code,
            timed_out,
            ..
        } => (true, exit_code, timed_out),
    };
    let tail = locked(&found.output);
    let total = tail.base + tail.bytes.len() as u64;
    if offset > total {
        return Err(OpError::invalid(format!(
            "offset {offset} is past the end of the output ({total} bytes)"
        )));
    }
    // Where the output starts: the offset, or the oldest byte still kept. A start inside a character (a cut in
    // the tail, or an offset given mid-character) moves on to the next one.
    let mut skip = (offset.max(tail.base) - tail.base) as usize;
    let limit = skip + 3;
    while skip < tail.bytes.len() && skip < limit && is_continuation(tail.bytes[skip]) {
        skip += 1;
    }
    let available = &tail.bytes[skip..];
    let start = tail.base + skip as u64;
    let reply = |chunk: &[u8]| RunPoll {
        done,
        exit_code,
        next: start + chunk.len() as u64,
        offset: start,
        output: String::from_utf8_lossy(chunk).into_owned(),
        run: run.to_string(),
        timed_out,
    };
    let end = byte_chunk_end(available, |end| {
        serde_json::to_string(&reply(&available[..end]))
            .is_ok_and(|json| json.len() <= OUTPUT_BUDGET)
    });
    Ok(reply(&available[..end]))
}

/// Asks the run to stop and waits briefly for it to end. The platform supervisor sees the flag and ends the run.
pub(crate) fn stop_run(registry: &Mutex<Registry>, run: &str) -> Result<RunStopped, OpError> {
    let found = lookup(registry, run)?;
    let was_running = found.is_running();
    found.stop.store(true, Ordering::SeqCst);
    let deadline = Instant::now() + RUN_STOP_WAIT;
    while found.is_running() && Instant::now() < deadline {
        thread::sleep(RUN_POLL);
    }
    Ok(RunStopped {
        done: !found.is_running(),
        run: run.to_string(),
        stopped: was_running,
    })
}

impl Run {
    /// Appends output to the tail, dropping the oldest bytes past [`OUTPUT_CAP`].
    pub(crate) fn append(&self, bytes: &[u8]) {
        let mut tail = locked(&self.output);
        tail.bytes.extend_from_slice(bytes);
        if tail.bytes.len() > OUTPUT_CAP {
            let dropped = tail.bytes.len() - OUTPUT_CAP;
            tail.bytes.drain(..dropped);
            tail.base += dropped as u64;
        }
    }

    /// Records how the run ended.
    pub(crate) fn finish(&self, exit_code: Option<i32>, timed_out: bool) {
        *locked(&self.state) = RunState::Done {
            exit_code,
            timed_out,
            at: Instant::now(),
        };
    }

    pub(crate) fn is_running(&self) -> bool {
        matches!(*locked(&self.state), RunState::Running)
    }

    /// Finished, and longer ago than [`RUN_KEEP`].
    fn expired(&self, now: Instant) -> bool {
        matches!(*locked(&self.state), RunState::Done { at, .. } if now.duration_since(at) >= RUN_KEEP)
    }

    fn finished_at(&self) -> Option<Instant> {
        match *locked(&self.state) {
            RunState::Running => None,
            RunState::Done { at, .. } => Some(at),
        }
    }
}

pub(crate) fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
