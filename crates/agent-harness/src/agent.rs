//! The model-driven agent loop (v3), run inside a keepalive session.
//!
//! A welcome carrying a `profile` (or a later `profile` line) makes the session
//! model-backed. `work` items queue FIFO and run one at a time. A work item is a
//! loop of model calls: the guest sends `model_call` with the conversation, the
//! host answers `model_result`, and each turn's tools are settled together: the
//! ones the host's signed `policy` allows run here and are reported under that
//! policy, and the rest go to the host in one `tool_batch`, answered once by
//! `tool_batch_decision` (allowed and run here, decided by the host, or denied).
//! The conversation only grows. A chat thread keeps its conversation
//! across work items until it passes [`THREAD_CLOSE_BYTES`].
//!
//! Every wait is one loop ([`Agent::await_message`]) that keeps beating, queues
//! work and records cancels while it waits. The host's lines arrive through the
//! reader lane ([`crate::lane`]), which also answers the owner's operator
//! `tool_call`s on a lane of their own. A local tool runs on its own thread that
//! the same beat runs beside, so a slow tool does not silence the session.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Write;
use std::num::NonZeroUsize;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use apz_proto::chat::{Block, ChatRequest, ChatResponse, Message, Role, StopReason, blocks_text};
use apz_proto::line::Lines;
use apz_proto::wire::{
    AgentProfile, BatchCall, BatchDecision, Control, Decision, EngineSpec, GuestMessage,
    HostMessage, MAX_TOOL_OUTPUT, PolicySnapshot, Program, WorkKind, WorkStatus, cap_output,
};
use serde_json::{Value, json};

use crate::acp::hub::HubLink;
use crate::bash;
use crate::exec;
use crate::files::{self, Workspace};
use crate::helpers::{self, Board};
use crate::lane::{Duplex, Inbound, OPERATOR_EXPIRY, Shared, read_host, serve_operators};
use crate::script;
use crate::session::{SessionError, escape_for_transcript, kind_of};
use crate::tool_batch::{self, Streamed, Touch};
use crate::tool_search::{self, Deferred};
use crate::tools;
use crate::v2;
use crate::{checkpoints, library, plan};

/// A chat thread whose history serializes past this is closed: its next work
/// starts a new conversation, opened with the thread's last summary.
pub const THREAD_CLOSE_BYTES: usize = 400 * 1024;
/// Consecutive identical failing tool calls that stop a work item.
const STUCK_FAILURES: u32 = 3;
/// Token-limit stops a work item continues past (with a "continue" message).
const TOKEN_LIMIT_CONTINUES: u32 = 1;
/// How many times a step session that ends its turn without a handover is asked for one before it ends without.
const HANDOVER_ASKS: u32 = 2;
/// What a step's output says when its task is finished and nothing is left for another session: no handover is asked.
const TASK_DONE: &str = "TASK DONE";
/// What the harness asks a step session that ended its turn without a handover (journal.rs, 9.2).
const HANDOVER_ASK: &str = "Write your handover with handover_write first: what stands, what is done, what is next, what to check first. Then end your turn. If the task is finished and nothing is left for another session, end with the line TASK DONE instead.";
/// How often a local tool's thread is checked while the beat runs beside it.
const LOCAL_POLL: Duration = Duration::from_millis(20);
/// How long a script's calls that arrive together wait to be decided as one batch.
const SCRIPT_GATHER: Duration = Duration::from_millis(5);
/// How long queued work waits for the profile. A hub restart can welcome the agent before its
/// profile is ready, and the profile follows a few seconds later.
const PROFILE_WAIT: Duration = Duration::from_secs(60);

/// A local tool's result: its output, or why it failed.
type Outcome = Result<String, String>;

/// A call's run, owned, so that it runs on a thread of its own.
type Thunk<'a> = Box<dyn FnOnce() -> Outcome + Send + 'a>;

/// A call's outcome and how long the call ran, in milliseconds (the tool's time, as the report carries it).
struct Timed {
    outcome: Outcome,
    ms: u64,
}

impl Timed {
    /// Runs `run` and times it.
    fn run(run: impl FnOnce() -> Outcome) -> Timed {
        let started = Instant::now();
        let outcome = run();
        Timed {
            outcome,
            ms: started.elapsed().as_millis() as u64,
        }
    }
}

/// How the policy and the hub settle one of a model turn's tool calls.
enum Route {
    /// Refused for its input: the text the model sees, without asking the hub.
    Refused(String),
    /// Answered here, not run: `tool_search`'s text, which the model sees as a success.
    Answered(String),
    /// Allowed by the policy with this id: it runs here.
    Local(String),
    /// Left to the hub, in the turn's batch.
    Hub,
}

/// How an allowed call runs: the program or the engine's launch spec its decision carries, and the policy the
/// harness decided it under (None for a call the hub decided).
#[derive(Clone, Default)]
struct Allowance {
    program: Option<Program>,
    engine: Option<EngineSpec>,
    policy: Option<String>,
}

/// One of a turn's calls: a result the model gets as it stands, or a call that runs here.
enum Step {
    /// Settled without running: the result text the model sees, and whether it is an error.
    Settled(String, bool),
    Run(Run),
}

/// A call that runs here: the call, what it touches, how it runs, and the `run_script` call it belongs to when a script
/// made it.
#[derive(Clone)]
struct Run {
    id: String,
    name: String,
    input: Value,
    touch: Touch,
    allowance: Allowance,
    script: Option<String>,
}

/// A turn's calls as they settle. Each result goes back in the model's order, and each run is reported once the calls
/// before it have settled.
struct Turn {
    steps: Vec<Step>,
    /// Each run's outcome, once it has one.
    outcomes: Vec<Option<Timed>>,
    /// The results settled so far, in the model's order.
    results: Vec<(String, bool)>,
}

/// A read-only call that ran while its answer streamed, kept for the answer's tool turn: the policy it ran under, and
/// the call it ran for (its tool and input) and its outcome.
struct Early {
    policy: String,
    name: String,
    input: Value,
    timed: Timed,
}

/// A read-only call still running while its answer streams.
struct Running<'scope> {
    id: String,
    policy: String,
    name: String,
    input: Value,
    thread: thread::ScopedJoinHandle<'scope, Timed>,
}

/// The report a guest sends once it has run a call. A call the policy decided carries its record: its tool, its
/// input and the policy it was decided under. It carries how long the call ran.
pub(crate) fn tool_report(
    work: u64,
    call: &str,
    ok: bool,
    output: String,
    record: Option<&(String, String, Value)>,
    ms: u64,
    script: Option<&str>,
) -> GuestMessage {
    let (policy, name, input) = match record {
        Some((policy, name, input)) => (
            Some(policy.clone()),
            Some(name.clone()),
            Some(input.clone()),
        ),
        None => (None, None, None),
    };
    GuestMessage::ToolReport {
        work,
        call: call.to_string(),
        ok,
        output,
        policy,
        name,
        input,
        duration_ms: Some(ms),
        script: script.map(str::to_string),
    }
}

/// A queued work item.
struct Job {
    work: u64,
    kind: WorkKind,
    thread: String,
    prompt: String,
    /// The task's lane in Weft.
    lane: Option<apz_proto::wire::WorkLane>,
}

/// A chat thread's conversation, kept between its work items.
#[derive(Default)]
struct Thread {
    messages: Vec<Message>,
    /// The summary of the thread's last work item.
    summary: String,
    /// When the thread was last stored, on the agent's clock: the oldest go first.
    at: u64,
}

/// What one turn of the read loop saw.
enum Incoming {
    /// The beat came due before a line; a heartbeat went out.
    Beat,
    /// A line the loop acted on itself (queued work, cancel, profile, control).
    Handled,
    /// The host stopped the agent.
    Stop(String),
    /// A line a wait may be waiting for.
    Message(HostMessage),
}

/// Why a wait ended early: the host stopped the agent, or the session broke.
enum Halt {
    Stop(String),
    Failed(SessionError),
}

impl From<SessionError> for Halt {
    fn from(error: SessionError) -> Self {
        Halt::Failed(error)
    }
}

/// The agent's state for one keepalive connection.
struct Agent<'a, 'l, S, W> {
    shared: &'a Shared<'l, S, W>,
    /// The host's messages the reader lane passes on.
    inbox: Receiver<Inbound>,
    workspace: &'a Workspace,
    heartbeat: Duration,
    next_beat: Instant,
    seq: u64,
    /// The last model call's request id.
    req: u64,
    profile: Option<AgentProfile>,
    /// The work item's deferred tools, and which of them the model has fetched.
    deferred: Deferred,
    /// The hub's signed policy: the tools this agent runs without asking. None until the hub sends one, and then
    /// every call goes to the hub.
    policy: Option<PolicySnapshot>,
    /// The read-only calls that ran while the last model answer streamed, by call id. The answer's tool turn uses each
    /// one the answer asks for, under the same policy; the others are not used.
    early: HashMap<String, Early>,
    queue: VecDeque<Job>,
    threads: HashMap<String, Thread>,
    /// Orders the stored threads: the next value a stored thread takes.
    clock: u64,
    /// The work item in progress, if any.
    current: Option<u64>,
    /// A cancel for the work item in progress, taken at its next step.
    cancelled: Option<String>,
    /// Steering notes (from, text) for the work item in progress, added to the
    /// conversation before its next model call.
    steers: Vec<(String, String)>,
    /// The operator asked for the work item in progress to be compressed.
    compress_requested: bool,
    /// The lane of the work in flight (Weft).
    lane: Option<apz_proto::wire::WorkLane>,
    /// The conversation's size right after the last compression of this work item: an automatic
    /// compression waits until it has grown by a quarter of `compress_at` since.
    compressed_size: u64,
    /// The context the work item's latest model call used, in tokens (its input and cache reads and
    /// writes). After a compression it waits for the next call's count.
    context_used: u64,
    /// Model calls the operator allowed beyond the profile's turn budget.
    extra_turns: u32,
    /// Set while a local tool runs, once the work item is cancelled or the agent is stopping: an
    /// engine session reads it and stops at once, instead of at the tool's end.
    tool_cancel: Arc<AtomicBool>,
    /// A stop that arrived while a local tool ran. The loop takes it at its next wait, as it
    /// would have taken the line had it arrived then.
    stopping: Option<String>,
    /// The helpers this connection runs (see `helpers`): started and awaited through the agent's own tools.
    helpers: Arc<Board>,
    /// When the next stats report is due.
    next_stats: Instant,
    /// Each process's CPU ticks at the last stats report, by pid, and when that report was taken.
    ticks: HashMap<u32, u64>,
    ticked_at: Instant,
}

/// How often the harness sends the host a stats report: the workspace's use and the busiest processes.
const STATS_EVERY: Duration = Duration::from_secs(60);

/// How many processes a stats report names.
const TOP_PROCESSES: usize = 5;

/// The process table the stats are read from.
const PROC_DIR: &str = "/proc";

/// The CPU ticks `/proc` counts in a second (USER_HZ), which is 100 on every Linux ABI.
const CLOCK_TICKS_PER_SECOND: f64 = 100.0;

/// One process as `/proc` showed it: its pid, its name, its CPU ticks so far and its resident memory.
#[derive(Debug, Clone, PartialEq)]
struct ProcSample {
    pid: u32,
    name: String,
    ticks: u64,
    rss_mib: f64,
}

/// The name and CPU ticks (user plus system) of a `/proc/<pid>/stat` line. The name is in
/// parentheses and may hold spaces or parentheses, so the fields are read after the last `)`.
fn parse_stat(stat: &str) -> Option<(String, u64)> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat.get(open + 1..close)?.to_string();
    let fields: Vec<&str> = stat.get(close + 1..)?.split_whitespace().collect();
    // The state is field 3, the first after the name, so utime (field 14) is the 12th and stime the 13th.
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some((name, utime + stime))
}

/// The resident memory in KiB from a `/proc/<pid>/status` file's `VmRSS` line, if it has one.
fn parse_rss_kib(status: &str) -> Option<u64> {
    let line = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?;
    line.split_whitespace().next()?.parse().ok()
}

/// Every process under `proc_dir` that could be read; one that exits mid-read is skipped.
fn sample_processes(proc_dir: &Path) -> Vec<ProcSample> {
    let Ok(entries) = fs::read_dir(proc_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let stat = fs::read_to_string(entry.path().join("stat")).ok()?;
            let (name, ticks) = parse_stat(&stat)?;
            let status = fs::read_to_string(entry.path().join("status")).unwrap_or_default();
            let rss_kib = parse_rss_kib(&status).unwrap_or(0);
            Some(ProcSample {
                pid,
                name,
                ticks,
                rss_mib: rss_kib as f64 / 1024.0,
            })
        })
        .collect()
}

/// The `count` busiest processes as (name, CPU %, RSS MiB), by CPU since the previous report. CPU is
/// the ticks a process gained over `elapsed_secs`; one new since then counts all its ticks.
fn busiest(
    previous: &HashMap<u32, u64>,
    now: &[ProcSample],
    elapsed_secs: f64,
    count: usize,
) -> Vec<(String, f64, f64)> {
    let mut rows: Vec<(String, f64, f64)> = now
        .iter()
        .map(|process| {
            let before = previous.get(&process.pid).copied().unwrap_or(0);
            let gained = process.ticks.saturating_sub(before) as f64;
            let cpu = if elapsed_secs > 0.0 {
                gained / CLOCK_TICKS_PER_SECOND / elapsed_secs * 100.0
            } else {
                0.0
            };
            (process.name.clone(), cpu, process.rss_mib)
        })
        .collect();
    rows.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.2.total_cmp(&a.2)));
    rows.truncate(count);
    rows
}

/// The MiB in use and the MiB in all on the filesystem at `path`, from `statvfs`.
fn workspace_use(path: &Path) -> Option<(u64, u64)> {
    const MIB: u64 = 1 << 20;
    let stat = rustix::fs::statvfs(path).ok()?;
    let size = stat.f_blocks * stat.f_frsize;
    let used = stat.f_blocks.saturating_sub(stat.f_bfree) * stat.f_frsize;
    Some((used / MIB, size / MIB))
}

/// Serves one keepalive connection's agent loop until the host's `stop`, then
/// sends the `bye`; returns how many operator tool calls were answered.
///
/// The reader lane owns the connection's read half for the whole session, and the
/// operator worker answers the owner's calls beside the loop. The `bye` goes out
/// only after both have finished, so every answer precedes it and the count is final.
pub fn serve<S: Duplex, W: Write + Send>(
    lines: &mut Lines<S>,
    log: &mut W,
    workspace: &Workspace,
    profile: Option<AgentProfile>,
    heartbeat: Duration,
) -> Result<u64, SessionError> {
    let cap = helpers::cap_for(thread::available_parallelism().map_or(1, NonZeroUsize::get));
    serve_capped(lines, log, workspace, profile, heartbeat, cap)
}

/// [`serve`], with the most helpers that run at once given (see `helpers::cap_for`).
pub(crate) fn serve_capped<S: Duplex, W: Write + Send>(
    lines: &mut Lines<S>,
    log: &mut W,
    workspace: &Workspace,
    profile: Option<AgentProfile>,
    heartbeat: Duration,
    cap: usize,
) -> Result<u64, SessionError> {
    let (board, helper_events) = Board::new(cap);
    let writer = Lines::new(lines.get_ref().duplicate()?);
    let shared = Shared::new(writer, log);
    // The first stats report measures CPU from here, so the baseline is taken now.
    let baseline = sample_processes(Path::new(PROC_DIR));
    // The profile a session starts with carries the project's hooks too (hooks.rs).
    if let Some(profile) = &profile {
        crate::hooks::set_for(workspace, profile.hooks.clone());
    }
    let (inbox_tx, inbox_rx) = mpsc::channel();
    let (operator_tx, operator_rx) = mpsc::channel();
    let (exec_tx, exec_rx) = mpsc::channel();
    let shared_ref = &shared;
    let outcome = thread::scope(|scope| {
        scope.spawn(move || read_host(lines, shared_ref, inbox_tx, operator_tx, exec_tx));
        scope.spawn(move || exec::serve(exec_rx, shared_ref, workspace));
        let dispatch_board = Arc::clone(&board);
        scope.spawn(move || {
            helpers::dispatch(scope, helper_events, dispatch_board, shared_ref, workspace)
        });
        scope.spawn(move || {
            let exec = |name: &str, args: &Value| match name {
                "read" | "edit" | "find" | "search" | "run" | "run_tests" => {
                    v2::serve(workspace, OPERATOR_WORK, name, args).map_err(|err| err.to_string())
                }
                _ => tools::serve(workspace, name, args).map_err(|err| err.to_string()),
            };
            serve_operators(operator_rx, shared_ref, exec, OPERATOR_EXPIRY)
        });
        let mut agent = Agent {
            shared: shared_ref,
            inbox: inbox_rx,
            workspace,
            heartbeat,
            next_beat: Instant::now() + heartbeat,
            seq: 0,
            req: 0,
            profile,
            deferred: Deferred::default(),
            policy: None,
            early: HashMap::new(),
            queue: VecDeque::new(),
            threads: load_threads(workspace),
            clock: 0,
            current: None,
            cancelled: None,
            steers: Vec::new(),
            compress_requested: false,
            lane: None,
            compressed_size: 0,
            context_used: 0,
            extra_turns: 0,
            tool_cancel: Arc::new(AtomicBool::new(false)),
            stopping: None,
            helpers: Arc::clone(&board),
            next_stats: Instant::now() + STATS_EVERY,
            ticks: baseline.iter().map(|p| (p.pid, p.ticks)).collect(),
            ticked_at: Instant::now(),
        };
        agent.clock = agent
            .threads
            .values()
            .map(|thread| thread.at)
            .max()
            .unwrap_or(0);
        let outcome = agent.run();
        board.close();
        shared_ref.stop();
        outcome
    });
    outcome?;
    let calls = shared.calls();
    shared.say(&GuestMessage::Bye { calls })?;
    Ok(calls)
}

impl<'a, 'l, S: Write + Send, W: Write + Send> Agent<'a, 'l, S, W> {
    /// The agent loop: runs queued work and waits for the host, until the host stops it.
    /// Work does not start before the profile arrives; it waits up to `PROFILE_WAIT`, and
    /// then fails as having no model.
    fn run(&mut self) -> Result<(), SessionError> {
        // When the queued work first found no profile.
        let mut waiting_since: Option<Instant> = None;
        loop {
            let halt = if self.queue.is_empty() {
                waiting_since = None;
                self.wait_for_host()
            } else if self.profile.is_some() {
                waiting_since = None;
                self.run_next()
            } else if waiting_since.get_or_insert_with(Instant::now).elapsed() < PROFILE_WAIT {
                self.wait_for_host()
            } else {
                // Still no profile: `run_job` fails the work item.
                self.run_next()
            };
            match halt {
                None => {}
                Some(Halt::Stop(reason)) => {
                    self.end(&reason);
                    return Ok(());
                }
                Some(Halt::Failed(error)) => return Err(error),
            }
        }
    }

    /// Runs the next queued work item; `None` when the queue is empty.
    fn run_next(&mut self) -> Option<Halt> {
        let job = self.queue.pop_front()?;
        self.run_job(job).err()
    }

    /// Waits for the host's next message or the heartbeat, as the loop does when idle.
    fn wait_for_host(&mut self) -> Option<Halt> {
        match self.pump() {
            Ok(Incoming::Stop(reason)) => Some(Halt::Stop(reason)),
            Ok(_) => None,
            Err(error) => Some(Halt::Failed(error)),
        }
    }

    /// Records the host's stop; the `bye` follows once the lanes have drained.
    fn end(&self, reason: &str) {
        let reason = if reason.is_empty() {
            "no reason given"
        } else {
            reason
        };
        self.shared.note(&format!(
            "agent-harness: the hub stopped the agent ({})",
            escape_for_transcript(reason)
        ));
    }

    fn say(&self, message: &GuestMessage) -> Result<(), SessionError> {
        self.shared.say(message)
    }

    /// Sends a heartbeat, saying whether a work item is in progress.
    fn beat(&mut self) -> Result<(), SessionError> {
        self.seq += 1;
        let seq = self.seq;
        let busy = self.current.is_some();
        self.say(&GuestMessage::Heartbeat {
            seq,
            busy: Some(busy),
        })?;
        self.shared.note(&format!("agent-harness: heartbeat {seq}"));
        self.next_beat = Instant::now() + self.heartbeat;
        Ok(())
    }

    /// Sends a stats report: the workspace's use and the busiest processes since the last report.
    /// It goes out only while the harness runs, so it never keeps the VM awake.
    fn report_stats(&mut self) -> Result<(), SessionError> {
        let now = Instant::now();
        let samples = sample_processes(Path::new(PROC_DIR));
        let elapsed = now.saturating_duration_since(self.ticked_at).as_secs_f64();
        let top = busiest(&self.ticks, &samples, elapsed, TOP_PROCESSES);
        self.ticks = samples.iter().map(|p| (p.pid, p.ticks)).collect();
        self.ticked_at = now;
        self.next_stats = now + STATS_EVERY;
        // A workspace that cannot be measured is left out rather than reported as empty.
        if let Some((used_mib, size_mib)) = workspace_use(Path::new(files::WORKSPACE_ROOT)) {
            self.say(&GuestMessage::Stats {
                workspace_used_mib: used_mib,
                workspace_size_mib: size_mib,
                top,
            })?;
        }
        Ok(())
    }

    /// Takes the next message from the reader lane, or beats when the beat came
    /// due. The messages the loop owns are acted on here; anything else is returned
    /// for a waiting caller.
    fn pump(&mut self) -> Result<Incoming, SessionError> {
        if let Some(reason) = self.stopping.take() {
            return Ok(Incoming::Stop(reason));
        }
        let wait = self
            .next_beat
            .min(self.next_stats)
            .saturating_duration_since(Instant::now());
        let message = match self.inbox.recv_timeout(wait) {
            Ok(Inbound::Message(message)) => message,
            Ok(Inbound::Closed) | Err(RecvTimeoutError::Disconnected) => {
                return Err(SessionError::HostClosed);
            }
            Ok(Inbound::Failed(err)) => return Err(SessionError::Io(err)),
            Err(RecvTimeoutError::Timeout) => {
                // The stats report and the beat come due at their own times; one that is not due
                // waits for the next wait, which is a moment away at most.
                if Instant::now() >= self.next_stats {
                    self.report_stats()?;
                } else {
                    self.beat()?;
                }
                return Ok(Incoming::Beat);
            }
        };
        self.take(message)
    }

    /// Acts on one host message the loop owns; anything else is returned for a waiting caller.
    fn take(&mut self, message: HostMessage) -> Result<Incoming, SessionError> {
        Ok(match message {
            HostMessage::Stop { reason } => {
                // A stop ends the agent's background shells too; a cancel of one work item
                // leaves them running.
                v2::stop_all(self.workspace);
                Incoming::Stop(reason)
            }
            HostMessage::Work {
                work,
                kind,
                thread,
                prompt,
                lane,
            } => {
                self.queue.push_back(Job {
                    work,
                    kind,
                    thread,
                    prompt,
                    lane,
                });
                Incoming::Handled
            }
            HostMessage::Cancel { work, reason } => {
                self.cancel(work, reason)?;
                Incoming::Handled
            }
            HostMessage::Profile { profile } => {
                // A changed profile can add or drop the egress CA (the owner edited a rule).
                crate::trust::install(self.workspace, profile.egress_ca.as_deref());
                crate::hooks::set_for(self.workspace, profile.hooks.clone());
                self.profile = Some(profile);
                Incoming::Handled
            }
            HostMessage::Policy { policy } => {
                self.policy = Some(policy);
                Incoming::Handled
            }
            HostMessage::Control { work, control } => {
                if self.current == Some(work) {
                    match control {
                        Control::Compress => self.compress_requested = true,
                        Control::Steer { from, text } => self.steers.push((from, text)),
                        Control::ExtendTurns { turns } => {
                            self.extra_turns = self.extra_turns.saturating_add(turns)
                        }
                    }
                } else {
                    self.shared.note(&format!(
                        "agent-harness: ignoring control for work {work}, which is not running"
                    ));
                }
                Incoming::Handled
            }
            other => Incoming::Message(other),
        })
    }

    /// Waits for the host message `pick` accepts; anything else is logged and
    /// skipped. A stop from the host ends the wait with [`Halt::Stop`].
    /// Waits `wait` while the host holds this work's model calls, beating and taking the host's messages meanwhile. A
    /// cancel ends the wait at once (the drive loop then stops the work).
    fn wait_paused(&mut self, wait: Duration) -> Result<(), Halt> {
        let until = Instant::now() + wait;
        while self.cancelled.is_none() && Instant::now() < until {
            match self.pump()? {
                Incoming::Stop(reason) => return Err(Halt::Stop(reason)),
                Incoming::Message(other) => {
                    let kind = kind_of(&other);
                    self.shared
                        .note(&format!("agent-harness: ignoring {kind} while paused"));
                }
                Incoming::Beat | Incoming::Handled => {}
            }
        }
        Ok(())
    }

    fn await_message<T>(
        &mut self,
        awaited: &str,
        mut pick: impl FnMut(HostMessage) -> Result<T, HostMessage>,
    ) -> Result<T, Halt> {
        loop {
            match self.pump()? {
                Incoming::Message(message) => match pick(message) {
                    Ok(picked) => return Ok(picked),
                    Err(other) => {
                        let kind = kind_of(&other);
                        self.shared.note(&format!(
                            "agent-harness: ignoring {kind} while waiting for {awaited}"
                        ));
                    }
                },
                Incoming::Stop(reason) => return Err(Halt::Stop(reason)),
                Incoming::Beat | Incoming::Handled => {}
            }
        }
    }

    /// Sends the heartbeat and the stats report when either is due.
    fn beat_when_due(&mut self) -> Result<(), SessionError> {
        if Instant::now() >= self.next_beat {
            self.beat()?;
        }
        if Instant::now() >= self.next_stats {
            self.report_stats()?;
        }
        Ok(())
    }

    /// Runs `jobs` on as many threads as the VM has CPUs, beating and taking the host's messages meanwhile. `done` gets
    /// each job's index and outcome as its job finishes. A job that panics is an error result, not a dead session.
    fn run_jobs(
        &mut self,
        jobs: Vec<(usize, Thunk<'a>)>,
        done: &mut dyn FnMut(&mut Self, usize, Timed) -> Result<(), SessionError>,
    ) -> Result<(), SessionError> {
        let total = jobs.len();
        if total == 0 {
            return Ok(());
        }
        let threads = thread::available_parallelism()
            .map_or(1, NonZeroUsize::get)
            .min(total);
        self.tool_cancel.store(false, Ordering::SeqCst);
        let queue = Mutex::new(VecDeque::from(jobs));
        let (finished, outcomes) = mpsc::channel();
        thread::scope(|scope| {
            for _ in 0..threads {
                let (queue, finished) = (&queue, finished.clone());
                scope.spawn(move || {
                    loop {
                        let next = queue
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .pop_front();
                        let Some((index, job)) = next else { break };
                        let timed = Timed::run(|| {
                            panic::catch_unwind(AssertUnwindSafe(job))
                                .unwrap_or_else(|_| Err("internal: the tool panicked".to_string()))
                        });
                        let _ = finished.send((index, timed));
                    }
                });
            }
            drop(finished);
            let mut seen = 0;
            while seen < total {
                match outcomes.recv_timeout(LOCAL_POLL) {
                    Ok((index, timed)) => {
                        seen += 1;
                        done(&mut *self, index, timed)?;
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        self.beat_when_due()?;
                        self.hear_during_tool()?;
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            Ok(())
        })
    }

    /// Takes the host's messages that arrived while a local tool runs, without
    /// waiting for them. A cancel of the work item reaches the tool through
    /// `tool_cancel`; a stop is kept for the loop's next wait, so the tool's result
    /// is still reported first.
    fn hear_during_tool(&mut self) -> Result<(), SessionError> {
        loop {
            let message = match self.inbox.try_recv() {
                Ok(Inbound::Message(message)) => message,
                Err(TryRecvError::Empty) => return Ok(()),
                Ok(Inbound::Closed) | Err(TryRecvError::Disconnected) => {
                    self.tool_cancel.store(true, Ordering::SeqCst);
                    return Err(SessionError::HostClosed);
                }
                Ok(Inbound::Failed(err)) => {
                    self.tool_cancel.store(true, Ordering::SeqCst);
                    return Err(SessionError::Io(err));
                }
            };
            match self.take(message)? {
                Incoming::Stop(reason) => {
                    self.tool_cancel.store(true, Ordering::SeqCst);
                    self.stopping = Some(reason);
                }
                Incoming::Message(other) => {
                    let kind = kind_of(&other);
                    self.shared
                        .note(&format!("agent-harness: ignoring {kind} while a tool runs"));
                }
                Incoming::Beat | Incoming::Handled => {}
            }
        }
    }

    /// `cancel`: the work item in progress stops after its current step; a
    /// queued one is finished as stopped without running.
    fn cancel(&mut self, work: u64, reason: String) -> Result<(), SessionError> {
        if self.current == Some(work) {
            self.cancelled = Some(reason);
            self.tool_cancel.store(true, Ordering::SeqCst);
        } else if let Some(at) = self.queue.iter().position(|job| job.work == work) {
            self.queue.remove(at);
            self.finish(work, WorkStatus::Stopped, &reason)?;
        } else {
            self.shared.note(&format!(
                "agent-harness: ignoring cancel for work {work}, which is not queued"
            ));
        }
        Ok(())
    }

    fn finish(&mut self, work: u64, status: WorkStatus, summary: &str) -> Result<(), SessionError> {
        self.shared.note(&format!(
            "agent-harness: work {work} {status:?}: {}",
            escape_for_transcript(summary)
        ));
        self.say(&GuestMessage::WorkDone {
            work,
            status,
            summary: summary.to_string(),
        })
    }

    /// Runs one work item to its end. A chat item extends its thread's
    /// conversation; any other item starts fresh.
    fn run_job(&mut self, job: Job) -> Result<(), Halt> {
        let Some(profile) = self.profile.clone() else {
            self.finish(job.work, WorkStatus::Failed, "this agent has no model")?;
            return Ok(());
        };
        let chat = job.kind == WorkKind::Chat;
        let mut prompt = job.prompt;
        let mut messages = Vec::new();
        if chat {
            let thread = self.threads.remove(&job.thread).unwrap_or_default();
            if serialized_len(&thread.messages) > THREAD_CLOSE_BYTES {
                prompt = format!(
                    "Earlier in this conversation: {}\n\n{prompt}",
                    thread.summary
                );
            } else {
                messages = thread.messages;
            }
        }
        // A conversation that ended on tool results (a stopped item) gets the
        // prompt in that same user turn, so roles still alternate.
        match messages.last_mut() {
            Some(last) if last.role == Role::User => {
                last.content.push(Block::Text { text: prompt })
            }
            _ => messages.push(Message::user_text(prompt)),
        }

        let _ = crate::checkpoints::snapshot(self.workspace, &format!("work-{}", job.work));
        // The task's lane: its files in the workspace, each turn kept as a draft in Weft.
        self.lane = job.lane.clone();
        if let Some(lane) = &self.lane
            && let Err(err) = crate::weft_lane::checkout(&self.workspace.weft_root(), lane)
        {
            self.shared.note(&format!(
                "agent-harness: lane {} not checked out: {err}",
                lane.change
            ));
        }
        self.current = Some(job.work);
        self.cancelled = None;
        self.steers.clear();
        self.compress_requested = false;
        self.compressed_size = 0;
        self.context_used = 0;
        self.extra_turns = 0;
        self.deferred = Deferred::new(&profile.tools);
        let result = self.drive(job.work, &profile, &mut messages);
        // A helper ends with its work item: one still running is stopped.
        self.helpers.cancel_work(job.work);
        // The last state, so the seal has it.
        if let Some(lane) = self.lane.take()
            && let Err(err) = crate::weft_lane::snapshot(
                &self.workspace.weft_root(),
                &lane,
                u32::MAX,
                &[],
                &profile.model,
            )
            && !err.contains("nothing new")
        {
            self.shared.note(&format!(
                "agent-harness: lane {} not snapshotted: {err}",
                lane.change
            ));
        }
        self.current = None;
        let (status, summary) = result?;
        if chat {
            self.clock += 1;
            let at = self.clock;
            self.threads.insert(
                job.thread,
                Thread {
                    messages,
                    summary: summary.clone(),
                    at,
                },
            );
            if let Err(err) = save_threads(self.workspace, &self.threads) {
                self.shared
                    .note(&format!("agent-harness: chat threads not saved: {err}"));
            }
        }
        self.finish(job.work, status, &summary)?;
        Ok(())
    }

    /// The model loop of one work item: call the model, run what it asks for,
    /// repeat until a stop condition. `messages` is extended in place.
    fn drive(
        &mut self,
        work: u64,
        profile: &AgentProfile,
        messages: &mut Vec<Message>,
    ) -> Result<(WorkStatus, String), Halt> {
        let mut turns = 0u32;
        let mut token_limit_hits = 0u32;
        // How many times in a row the host answered a model call "paused".
        let mut paused_waits = 0u32;
        // The last tool call's name and input, and how many times in a row it failed.
        let mut failing: Option<(String, u32)> = None;
        // Whether a handover was written in this work (a step session must write one), and how often it was asked.
        let mut handed_over = false;
        let mut handover_asks = 0u32;
        loop {
            if let Some(reason) = self.cancelled.take() {
                return Ok((WorkStatus::Stopped, reason));
            }
            for (from, text) in std::mem::take(&mut self.steers) {
                let note_text = format!("[note from {from}]: {text}");
                match messages.last_mut() {
                    Some(last) if last.role == Role::User => {
                        last.content.push(Block::Text { text: note_text })
                    }
                    _ => messages.push(Message::user_text(note_text)),
                }
            }
            let size = serialized_len(messages.as_slice()) as u64;
            // With the model's window known, the conversation compresses once its last call used `compact_at_pct` of
            // the window; without it, once it is larger than `compress_at` bytes and has grown by a quarter of that.
            let auto = if profile.context_window > 0 && profile.compact_at_pct > 0 {
                self.context_used.saturating_mul(100)
                    >= profile
                        .context_window
                        .saturating_mul(u64::from(profile.compact_at_pct))
            } else {
                profile.compress_at > 0
                    && size > profile.compress_at
                    && size > self.compressed_size.saturating_add(profile.compress_at / 4)
            };
            if self.compress_requested || auto {
                let reason = if self.compress_requested {
                    "asked"
                } else {
                    "auto"
                };
                self.compress_requested = false;
                // The next call's count measures the compressed conversation.
                self.context_used = 0;
                self.compress(work, profile, messages, reason)?;
            }
            if turns >= profile.max_turns.saturating_add(self.extra_turns) {
                return Ok((WorkStatus::Stopped, "turn budget reached".to_string()));
            }
            turns += 1;
            let response = match self.model_call(work, profile, messages)? {
                Ok(response) => {
                    paused_waits = 0;
                    response
                }
                // The host holds this work's model calls (its budget or a limit is reached, the plaza is held): the
                // turn waits and asks again, instead of failing; a cancel ends the wait.
                Err(reason) if reason.starts_with(PAUSED) => {
                    turns -= 1;
                    let wait = paused_wait(paused_waits);
                    paused_waits += 1;
                    self.shared.note(&format!(
                        "agent-harness: {reason}; asking again in {}s",
                        wait.as_secs()
                    ));
                    self.wait_paused(wait)?;
                    continue;
                }
                Err(reason) => return Ok((WorkStatus::Failed, reason)),
            };
            let ChatResponse {
                content,
                stop,
                usage,
                ..
            } = response;
            self.context_used =
                usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens;
            messages.push(Message::assistant(content.clone()));

            let uses: Vec<(String, String, Value, Option<Value>)> = content
                .iter()
                .filter_map(|block| match block {
                    Block::ToolUse {
                        id,
                        name,
                        input,
                        meta,
                    } => Some((id.clone(), name.clone(), input.clone(), meta.clone())),
                    _ => None,
                })
                .collect();
            let ran_tools = !uses.is_empty();
            let mut results = Vec::new();
            let mut stuck = None;
            let outcomes = self.tool_turn(work, &uses)?;
            for ((id, name, input, _), (text, is_error)) in uses.into_iter().zip(outcomes) {
                if name == "handover_write" && !is_error {
                    handed_over = true;
                }
                let key = format!("{name} {input}");
                failing = match (is_error, failing.take()) {
                    (false, _) => None,
                    (true, Some((last, count))) if last == key => Some((last, count + 1)),
                    (true, _) => Some((key, 1)),
                };
                if matches!(failing, Some((_, count)) if count >= STUCK_FAILURES) && stuck.is_none()
                {
                    stuck = Some(format!(
                        "stuck: {name} failed {STUCK_FAILURES} times in a row with the same input"
                    ));
                }
                results.push(Block::ToolResult {
                    tool_use_id: id,
                    content: text,
                    is_error,
                });
            }
            if ran_tools {
                // This turn's files, as a draft of the task's lane, with the tool calls that made them.
                if let Some(lane) = &self.lane {
                    let ids: Vec<String> = results
                        .iter()
                        .filter_map(|b| match b {
                            Block::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
                            _ => None,
                        })
                        .collect();
                    if let Err(err) = crate::weft_lane::snapshot(
                        &self.workspace.weft_root(),
                        lane,
                        turns,
                        &ids,
                        &profile.model,
                    ) {
                        self.shared.note(&format!(
                            "agent-harness: lane {} turn {turns} not snapshotted: {err}",
                            lane.change
                        ));
                    }
                }
                messages.push(Message {
                    role: Role::User,
                    content: results,
                });
            }
            if let Some(reason) = stuck {
                return Ok((WorkStatus::Stopped, reason));
            }

            match stop {
                StopReason::EndTurn => {
                    let said = blocks_text(&content);
                    if profile.handover && !handed_over && !said.contains(TASK_DONE) {
                        if handover_asks < HANDOVER_ASKS {
                            handover_asks += 1;
                            match messages.last_mut() {
                                Some(last) if last.role == Role::User => {
                                    last.content.push(Block::Text {
                                        text: HANDOVER_ASK.to_string(),
                                    })
                                }
                                _ => messages.push(Message::user_text(HANDOVER_ASK)),
                            }
                            continue;
                        }
                        self.shared.note(&format!(
                            "agent-harness: work {work} ended without a handover"
                        ));
                    }
                    return Ok((WorkStatus::Done, said));
                }
                StopReason::Refusal => {
                    return Ok((WorkStatus::Failed, "the model declined".to_string()));
                }
                StopReason::MaxTokens => {
                    token_limit_hits += 1;
                    if token_limit_hits > TOKEN_LIMIT_CONTINUES {
                        return Ok((
                            WorkStatus::Stopped,
                            "the model hit its token limit".to_string(),
                        ));
                    }
                    if !ran_tools {
                        messages.push(Message::user_text("continue"));
                    }
                }
                other if !ran_tools => {
                    return Ok((
                        WorkStatus::Failed,
                        format!("the model stopped without an answer ({other:?})"),
                    ));
                }
                _ => {}
            }
        }
    }

    /// Folds the conversation's older turns into a summary the model writes: the
    /// first message (the task) stays, the `keep_recent` latest turns stay verbatim,
    /// and the summary is added to the first message. A failed summary is logged
    /// and the conversation goes on uncompressed.
    fn compress(
        &mut self,
        work: u64,
        profile: &AgentProfile,
        messages: &mut Vec<Message>,
        reason: &str,
    ) -> Result<(), Halt> {
        let budget = if profile.compress_at > 0 {
            (profile.compress_at / 2) as usize
        } else {
            usize::MAX
        };
        let Some(cut) = compress_cut(messages.as_slice(), profile.keep_recent, budget) else {
            return Ok(());
        };
        if cut <= 1 {
            return Ok(());
        }
        let before = serialized_len(messages.as_slice()) as u64;
        // The previous summary is folded into the new one, so the conversation carries exactly one.
        let previous: Vec<String> = messages[0]
            .content
            .iter()
            .filter_map(|block| match block {
                Block::Text { text } if text.starts_with(SUMMARY_HEAD) => Some(text.clone()),
                _ => None,
            })
            .collect();
        messages[0].content.retain(
            |block| !matches!(block, Block::Text { text } if text.starts_with(SUMMARY_HEAD)),
        );
        let mut transcript = previous.join("\n");
        if !transcript.is_empty() {
            transcript.push('\n');
        }
        for message in &messages[1..cut] {
            let role = if message.role == Role::User {
                "user"
            } else {
                "assistant"
            };
            for block in &message.content {
                let line = match block {
                    Block::Text { text } => text.clone(),
                    Block::ToolUse { name, input, .. } => format!("→ {name} {input}"),
                    Block::ToolResult { content, .. } => format!("← {}", cap_output(content, 2000)),
                    _ => continue,
                };
                transcript.push_str(&format!("{role}: {line}\n"));
            }
        }
        let request = format!(
            "Summarize this work so far for yourself, so you can continue it without the transcript. The transcript may start with an earlier summary: keep what it says that still holds. Write four parts, each as short points: Decisions (each with its reason); Dead ends (what was tried and why it failed); Open questions; Where it stands (the goal, what is done, files you created or changed (paths), and the next steps). Be specific and complete; at most 600 words.\n\n{transcript}"
        );
        let mut summary_profile = profile.clone();
        summary_profile.tools.clear();
        let summary_msgs = vec![Message::user_text(request)];
        let response = match self.model_call(work, &summary_profile, &summary_msgs)? {
            Ok(response) => response,
            Err(reason) => {
                let reason = escape_for_transcript(&reason);
                self.shared.note(&format!(
                    "agent-harness: work {work} not compressed: {reason}"
                ));
                return Ok(());
            }
        };
        let summary = blocks_text(&response.content);
        messages[0].content.push(Block::Text {
            text: format!("{SUMMARY_HEAD}\n{summary}"),
        });
        messages.drain(1..cut);
        clip_old_results(messages);
        let after = serialized_len(messages.as_slice()) as u64;
        self.compressed_size = after;
        self.shared.note(&format!(
            "agent-harness: work {work} compressed ({reason}): {before} -> {after} bytes"
        ));
        self.say(&GuestMessage::Compressed {
            work,
            before,
            after,
            folded: (cut - 1) as u32,
            reason: reason.to_string(),
            summary,
        })?;
        Ok(())
    }

    /// One model call on the conversation so far. The inner `Err` is the host's
    /// reason the call failed.
    fn model_call(
        &mut self,
        work: u64,
        profile: &AgentProfile,
        messages: &[Message],
    ) -> Result<Result<ChatResponse, String>, Halt> {
        self.req += 1;
        let req = self.req;
        let request = ChatRequest {
            model: String::new(),
            system: None,
            system_parts: profile.system_parts.clone(),
            messages: messages.to_vec(),
            tools: self.deferred.request_tools(&profile.tools),
            // The whole prefix of the conversation is cached for the next call (PLAN 13.3).
            cache_last_message: true,
            max_tokens: 0,
            temperature: None,
            effort: None,
        };
        self.say(&GuestMessage::ModelCall {
            req,
            work,
            request,
            helper: None,
        })?;
        // The calls that ran ahead of an earlier answer are not this answer's.
        self.early.clear();
        let workspace = self.workspace;
        let (ok, response, error) = thread::scope(|scope| -> Result<_, Halt> {
            let mut streamed = Streamed::default();
            let mut running = Vec::new();
            let answer = loop {
                match self.pump()? {
                    // A read-only call whose block is whole starts now, if the policy allows it and no call before it
                    // could change what it reads (see `tool_batch::Streamed`).
                    Incoming::Message(HostMessage::ModelDelta { req: got, delta })
                        if got == req =>
                    {
                        let Some(whole) = streamed.push(workspace, delta) else {
                            continue;
                        };
                        // A deferred tool the model has not fetched runs in no way, not even ahead of the answer.
                        let allowed = self.policy.as_ref().filter(|policy| {
                            policy.allow.contains(&whole.name)
                                && self.deferred.refusal(&whole.name).is_none()
                        });
                        let Some(policy) = allowed.map(|policy| policy.id.clone()) else {
                            continue;
                        };
                        let (name, input) = (whole.name.clone(), whole.input.clone());
                        let (shared, call) = (self.shared, whole.id.clone());
                        let thread = scope.spawn(move || {
                            Timed::run(|| {
                                let outcome = run_guest_tool(workspace, work, &name, &input);
                                crate::anchors::surface(
                                    shared, workspace, work, &call, &name, &input, outcome,
                                )
                            })
                        });
                        running.push(Running {
                            id: whole.id,
                            policy,
                            name: whole.name,
                            input: whole.input,
                            thread,
                        });
                    }
                    Incoming::Message(HostMessage::ModelResult {
                        req: got,
                        ok,
                        response,
                        error,
                    }) if got == req => break (ok, response, error),
                    Incoming::Message(other) => {
                        let kind = kind_of(&other);
                        self.shared.note(&format!(
                            "agent-harness: ignoring {kind} while waiting for model_result"
                        ));
                    }
                    Incoming::Stop(reason) => return Err(Halt::Stop(reason)),
                    Incoming::Beat | Incoming::Handled => {}
                }
            };
            // The answer is in: the calls that ran ahead finish first, beating meanwhile.
            while running.iter().any(|call| !call.thread.is_finished()) {
                self.beat_when_due()?;
                self.hear_during_tool()?;
                thread::sleep(LOCAL_POLL);
            }
            for call in running {
                let timed = call.thread.join().unwrap_or_else(|_| Timed {
                    outcome: Err("internal: the tool panicked".to_string()),
                    ms: 0,
                });
                self.early.insert(
                    call.id,
                    Early {
                        policy: call.policy,
                        name: call.name,
                        input: call.input,
                        timed,
                    },
                );
            }
            Ok(answer)
        })?;
        Ok(match (ok, response) {
            (true, Some(response)) => Ok(response),
            (true, None) => Err("the host sent no answer".to_string()),
            (false, _) => Err(error.unwrap_or_else(|| "the model call failed".to_string())),
        })
    }

    /// The turn's tool calls, in the model's order, each as its result text and whether it is an error. A call with
    /// invalid input is refused without asking the host. A call the policy allows runs here, reported under the policy
    /// it was decided by. The others go to the host in one batch, which answers once. The calls that run here run in
    /// waves: the calls that touch nothing in common run at once, and the rest keep their order (see `tool_batch`).
    fn tool_turn(
        &mut self,
        work: u64,
        uses: &[(String, String, Value, Option<Value>)],
    ) -> Result<Vec<(String, bool)>, Halt> {
        let early = std::mem::take(&mut self.early);
        let steps = self.decide(work, uses, None)?;
        self.run_steps(work, steps, early)
    }

    /// How one call is routed: refused for its input or its tool, answered by `tool_search`, allowed by the policy (it
    /// runs here), or left to the hub. The same for a model's call and a script's.
    fn route(&mut self, name: &str, input: &Value, meta: Option<&Value>) -> Route {
        if let Some(reason) = invalid_input(input, meta) {
            Route::Refused(format!("INVALID_INPUT: {reason}"))
        } else if name == tool_search::TOOL {
            match input["query"].as_str() {
                Some(query) => Route::Answered(self.deferred.search(query)),
                None => Route::Refused("INVALID_INPUT: tool_search needs a query".into()),
            }
        } else if let Some(reason) = self.deferred.refusal(name) {
            Route::Refused(reason)
        } else {
            match &self.policy {
                Some(policy) if policy.allow.iter().any(|tool| tool == name) => {
                    Route::Local(policy.id.clone())
                }
                _ => Route::Hub,
            }
        }
    }

    /// Decides the calls of a turn, or of a batch a script made, in order: each call's step. A call the policy or the hub
    /// allows runs here. `script` is the `run_script` call the calls belong to, when a script made them; the calls are
    /// reported under it. The calls the hub decides go to it in one batch.
    fn decide(
        &mut self,
        work: u64,
        uses: &[(String, String, Value, Option<Value>)],
        script: Option<&str>,
    ) -> Result<Vec<Step>, Halt> {
        let routes: Vec<Route> = uses
            .iter()
            .map(|(_, name, input, meta)| self.route(name, input, meta.as_ref()))
            .collect();
        let calls: Vec<BatchCall> = uses
            .iter()
            .zip(&routes)
            .filter(|(_, route)| matches!(route, Route::Hub))
            .map(|((id, name, input, _), _)| BatchCall {
                call: id.clone(),
                name: name.clone(),
                input: input.clone(),
            })
            .collect();
        let mut decided: HashMap<String, BatchDecision> = HashMap::new();
        if !calls.is_empty() {
            self.say(&GuestMessage::ToolBatch { work, calls })?;
            let answers = self.await_message("tool_batch_decision", |message| match message {
                HostMessage::ToolBatchDecision {
                    work: got,
                    decisions,
                } if got == work => Ok(decisions),
                other => Err(other),
            })?;
            decided = answers
                .into_iter()
                .map(|decision| (decision.call.clone(), decision))
                .collect();
        }
        let steps = uses
            .iter()
            .zip(routes)
            .map(|((id, name, input, _), route)| {
                let step = match route {
                    Route::Refused(text) => Step::Settled(text, true),
                    Route::Answered(text) => Step::Settled(text, false),
                    Route::Local(policy) => run_step(
                        self.workspace,
                        id,
                        name,
                        input,
                        Allowance {
                            policy: Some(policy),
                            ..Allowance::default()
                        },
                    ),
                    Route::Hub => match decided.remove(id) {
                        Some(decision) => decided_step(self.workspace, id, name, input, decision),
                        None => Step::Settled(
                            "the host sent no decision for this tool call".to_string(),
                            true,
                        ),
                    },
                };
                match (step, script) {
                    (Step::Run(run), Some(script)) => Step::Run(Run {
                        script: Some(script.to_string()),
                        ..run
                    }),
                    (step, _) => step,
                }
            })
            .collect();
        Ok(steps)
    }

    /// Runs the steps of a turn or of a script's batch, and returns each call's result in order. A call that ran while the
    /// answer streamed (`early`) is not run again. The calls still to run go in waves: the calls that touch nothing in
    /// common run at once, and the rest keep their order (see `tool_batch`). A script runs alone: it touches the whole
    /// workspace (see `tool_batch::touch`), so no other call shares its wave.
    fn run_steps(
        &mut self,
        work: u64,
        steps: Vec<Step>,
        mut early: HashMap<String, Early>,
    ) -> Result<Vec<(String, bool)>, Halt> {
        let outcomes = steps
            .iter()
            .map(|step| match step {
                Step::Run(run) => early
                    .remove(&run.id)
                    .filter(|ran| {
                        ran.name == run.name
                            && ran.input == run.input
                            && run.allowance.policy.as_deref() == Some(ran.policy.as_str())
                    })
                    .map(|ran| ran.timed),
                Step::Settled(..) => None,
            })
            .collect();
        let mut turn = Turn {
            steps,
            outcomes,
            results: Vec::new(),
        };
        // The calls still to run, in waves (see `tool_batch::waves`). They are found before anything settles, since
        // settling takes the outcomes it reports: a call that ran ahead of the answer is not run again.
        let pending: Vec<usize> = (0..turn.steps.len())
            .filter(|&index| {
                matches!(turn.steps[index], Step::Run(_)) && turn.outcomes[index].is_none()
            })
            .collect();
        let touches: Vec<Touch> = pending
            .iter()
            .map(|&index| match &turn.steps[index] {
                Step::Run(run) => run.touch.clone(),
                Step::Settled(..) => Touch::Other,
            })
            .collect();
        self.settle(work, &mut turn)?;
        for wave in tool_batch::waves(&touches) {
            let mut jobs: Vec<(usize, Thunk<'a>)> = Vec::new();
            for &position in &wave {
                let index = pending[position];
                let Step::Run(run) = &turn.steps[index] else {
                    continue;
                };
                if run.name == script::TOOL {
                    let run = run.clone();
                    let timed = self.script_run(work, &run)?;
                    turn.outcomes[index] = Some(timed);
                    self.settle(work, &mut turn)?;
                } else {
                    jobs.push((index, self.job(work, run)));
                }
            }
            self.run_jobs(jobs, &mut |agent, index, timed| {
                turn.outcomes[index] = Some(timed);
                agent.settle(work, &mut turn)
            })?;
        }
        self.settle(work, &mut turn)?;
        Ok(turn.results)
    }

    /// Runs one `run_script` call, and returns its timed result: the script's value, or why it did not finish.
    fn script_run(&mut self, work: u64, run: &Run) -> Result<Timed, Halt> {
        let started = Instant::now();
        let outcome = self.script_drive(work, run)?;
        Ok(Timed {
            outcome,
            ms: started.elapsed().as_millis() as u64,
        })
    }

    /// Runs the script of one `run_script` call while it makes its calls: each batch of calls it makes is decided and
    /// run here (see `script_batch`), until the script ends, runs past its time or its output, or the work is cancelled.
    /// A host stop ends the wait with [`Halt::Stop`].
    fn script_drive(&mut self, work: u64, run: &Run) -> Result<Outcome, Halt> {
        let script = match script::Script::parse(&run.input) {
            Ok(script) => script,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let tools = self
            .profile
            .as_ref()
            .map_or_else(Vec::new, |profile| profile.tools.clone());
        let mut session = match script::Session::start(self.workspace, &script, &tools) {
            Ok(session) => session,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let (calls, requests) = mpsc::channel();
        let deadline = Instant::now() + Duration::from_secs(script.timeout_s);
        let mut made = 0;
        let mut stopped = loop {
            if let Some(reason) = self.stopping.take() {
                return Err(Halt::Stop(reason));
            }
            if self.cancelled.is_some() {
                break Some("the work item was cancelled; the script was killed".to_string());
            }
            if session.exited() {
                break None;
            }
            if Instant::now() >= deadline {
                break Some(format!(
                    "timed out after {} s; the script was killed",
                    script.timeout_s
                ));
            }
            if session.output_bytes() > script::MAX_OUTPUT_BYTES {
                break Some(output_cap_reason());
            }
            session.accept(&calls);
            match requests.recv_timeout(LOCAL_POLL) {
                Ok(first) => {
                    let mut batch = vec![first];
                    let gathered_until = Instant::now() + SCRIPT_GATHER;
                    while let Ok(next) = requests
                        .recv_timeout(gathered_until.saturating_duration_since(Instant::now()))
                    {
                        batch.push(next);
                    }
                    self.script_batch(work, &run.id, &mut made, batch)?;
                }
                Err(RecvTimeoutError::Timeout) => {
                    self.beat_when_due()?;
                    self.hear_during_tool()?;
                }
                Err(RecvTimeoutError::Disconnected) => break None,
            }
        };
        // A script that printed past the cap and ended before the look above is still past it.
        if stopped.is_none() && session.output_bytes() > script::MAX_OUTPUT_BYTES {
            stopped = Some(output_cap_reason());
        }
        Ok(session.finish(stopped))
    }

    /// Decides and runs one batch of the calls a script made, and answers each on its connection. A call past the
    /// script's cap, and a call to `run_script`, are refused here; the others are decided as `decide` says.
    fn script_batch(
        &mut self,
        work: u64,
        script_call: &str,
        made: &mut usize,
        batch: Vec<script::Request>,
    ) -> Result<(), Halt> {
        let mut answers: Vec<Option<(String, bool)>> = vec![None; batch.len()];
        let mut uses = Vec::new();
        let mut slots = Vec::new();
        for (slot, request) in batch.iter().enumerate() {
            *made += 1;
            if *made > script::MAX_CALLS {
                answers[slot] = Some((
                    format!(
                        "INVALID_INPUT: a script makes at most {} tool calls",
                        script::MAX_CALLS
                    ),
                    true,
                ));
            } else if request.name == script::TOOL {
                answers[slot] = Some(("a script cannot run a script".to_string(), true));
            } else {
                uses.push((
                    format!("{script_call}.{made}"),
                    request.name.clone(),
                    request.input.clone(),
                    None,
                ));
                slots.push(slot);
            }
        }
        if !uses.is_empty() {
            let steps = self.decide(work, &uses, Some(script_call))?;
            let results = self.run_steps(work, steps, HashMap::new())?;
            for (slot, result) in slots.into_iter().zip(results) {
                answers[slot] = Some(result);
            }
        }
        for (request, answer) in batch.into_iter().zip(answers) {
            if let Some((output, is_error)) = answer {
                request.answer(!is_error, &output);
            }
        }
        Ok(())
    }

    /// Settles the turn's calls in the model's order, as far as their outcomes are known: a call that ran is reported,
    /// and every result is kept for the model.
    fn settle(&mut self, work: u64, turn: &mut Turn) -> Result<(), SessionError> {
        while let Some(step) = turn.steps.get(turn.results.len()) {
            let index = turn.results.len();
            let result = match step {
                Step::Settled(text, is_error) => (text.clone(), *is_error),
                Step::Run(run) => match turn.outcomes[index].take() {
                    Some(timed) => self.report(work, run, timed)?,
                    None => break,
                },
            };
            turn.results.push(result);
        }
        Ok(())
    }

    /// Reports a call that ran here, and returns its result for the model. A call the policy decided carries its record
    /// in the report: its tool, its input and the policy it was decided under.
    fn report(
        &mut self,
        work: u64,
        run: &Run,
        timed: Timed,
    ) -> Result<(String, bool), SessionError> {
        let (ok, output) = match timed.outcome {
            Ok(output) => (true, output),
            Err(err) => (false, err),
        };
        let record = run
            .allowance
            .policy
            .clone()
            .map(|policy| (policy, run.name.clone(), run.input.clone()));
        // A packaged tool travels to the host whole (the library files it from the report);
        // the model only needs to hear what was packaged.
        if run.name == "tool_package" && ok {
            self.say(&tool_report(
                work,
                &run.id,
                ok,
                output.clone(),
                record.as_ref(),
                timed.ms,
                run.script.as_deref(),
            ))?;
            return Ok((library::summary(&output), false));
        }
        let output = cap_output(&output, MAX_TOOL_OUTPUT);
        self.say(&tool_report(
            work,
            &run.id,
            ok,
            output.clone(),
            record.as_ref(),
            timed.ms,
            run.script.as_deref(),
        ))?;
        Ok((output, !ok))
    }

    /// The job that runs `run` here: a library tool with its program, an engine's ACP session, or a built-in tool. The
    /// job owns what it needs, so it can run on a thread of its own.
    fn job(&self, work: u64, run: &Run) -> Thunk<'a> {
        let workspace = self.workspace;
        let name = run.name.clone();
        let input = run.input.clone();
        match run.allowance.program.clone() {
            Some(program) => Box::new(move || library::run(workspace, &program, &input)),
            // An engine runs as an ACP session whose every action the hub checks. The hub's launch spec, for an engine
            // instance the owner configured, comes with the decision.
            None if name == "code_engine" => {
                let shared = self.shared;
                let cancel = Arc::clone(&self.tool_cancel);
                let call = run.id.clone();
                let engine = run.allowance.engine.clone();
                // The engine's MCP server offers the profile's plaza tools, through the hub.
                let tools = self.profile.as_ref().map_or_else(Vec::new, |profile| {
                    profile
                        .tools
                        .iter()
                        .filter(|tool| tool.name != tool_search::TOOL)
                        .cloned()
                        .collect()
                });
                // The prompt parts the engine is told, as its instruction file (see `acp::adapter`).
                let instructions = self.profile.as_ref().map_or_else(Vec::new, |profile| {
                    profile
                        .system_parts
                        .iter()
                        .map(|part| part.text.clone())
                        .collect()
                });
                let protocol = self.profile.as_ref().map_or(1, |profile| {
                    crate::acp::run::protocol_offer(&profile.engine_transport)
                });
                Box::new(move || {
                    // The plaza's VM tools the hub lets run here (code intelligence, tests, library programs)
                    // run through the same path as the agent's own calls.
                    let here =
                        move |name: &str, input: &Value, program: Option<&Program>| match program {
                            Some(program) => library::run(workspace, program, input),
                            None => run_guest_tool(workspace, work, name, input),
                        };
                    let link = HubLink::new(shared, work, call, &cancel, tools)
                        .in_vm(here)
                        .with_instructions(instructions);
                    crate::acp::run::run(workspace, &input, &link, protocol, engine.as_ref())
                })
            }
            // A helper is started and awaited here: the board runs it on a thread of the agent's scope.
            None if name == "spawn_helper" => {
                let board = Arc::clone(&self.helpers);
                let profile = self.profile.clone();
                let policy = self.policy.clone();
                Box::new(move || match profile {
                    Some(profile) => board.spawn(work, &profile, policy, &input),
                    None => Err("this agent has no model".to_string()),
                })
            }
            None if name == "await_helper" => {
                let board = Arc::clone(&self.helpers);
                Box::new(move || board.await_report(&input))
            }
            None => {
                let shared = self.shared;
                let call = run.id.clone();
                Box::new(move || {
                    let outcome = run_guest_tool(workspace, work, &name, &input);
                    crate::anchors::surface(shared, workspace, work, &call, &name, &input, outcome)
                })
            }
        }
    }
}

/// Why a script was stopped for printing past its output cap.
fn output_cap_reason() -> String {
    format!(
        "printed more than {} KiB; the script was killed",
        script::MAX_OUTPUT_BYTES >> 10
    )
}

/// The step of a call that runs here: what it is, and what it touches.
fn run_step(
    workspace: &Workspace,
    id: &str,
    name: &str,
    input: &Value,
    allowance: Allowance,
) -> Step {
    Step::Run(Run {
        id: id.to_string(),
        name: name.to_string(),
        input: input.clone(),
        touch: tool_batch::touch(workspace, name, input),
        allowance,
        script: None,
    })
}

/// The step of a call the hub decided: an allowed call runs here under the hub's launch spec, and any other is
/// settled as its decision says.
fn decided_step(
    workspace: &Workspace,
    id: &str,
    name: &str,
    input: &Value,
    decided: BatchDecision,
) -> Step {
    let BatchDecision {
        decision,
        output,
        is_error,
        program,
        engine,
        ..
    } = decided;
    match decision {
        Decision::Allow => run_step(
            workspace,
            id,
            name,
            input,
            Allowance {
                program,
                engine,
                policy: None,
            },
        ),
        Decision::Done => Step::Settled(cap_output(&output, MAX_TOOL_OUTPUT), is_error),
        // Held for the owner (only an ACP engine's requests are held, and those wait on their own nested ids). A
        // model's call that comes back held is an error it can retry later.
        Decision::Pending => Step::Settled(
            "the owner has not decided this tool call yet; try again later".to_string(),
            true,
        ),
        Decision::Deny if output.is_empty() => {
            Step::Settled("the host denied this tool call".to_string(), true)
        }
        Decision::Deny => Step::Settled(output, true),
    }
}

/// Why a tool call's input cannot run, found without asking the host: the
/// gateway could not parse the model's arguments, or they are not an object.
pub(crate) fn invalid_input(input: &Value, meta: Option<&Value>) -> Option<&'static str> {
    if meta.is_some_and(|meta| {
        meta.get("kind").and_then(Value::as_str) == Some("gateway")
            && meta.get("error").and_then(Value::as_str) == Some("INVALID_JSON")
    }) {
        return Some("the arguments were not valid JSON");
    }
    if !input.is_object() {
        return Some("the arguments must be a JSON object");
    }
    None
}

/// The work id the owner's operator calls run under in the v2 tools: a work item of its own, so the reads and
/// shell of the agent's work items are not mixed with the owner's.
const OPERATOR_WORK: u64 = 0;

/// The names [`run_guest_tool`] serves itself, besides the tools of [`tools::serve`].
const GUEST_TOOLS: [&str; 25] = [
    "run_script",
    "read",
    "edit",
    "find",
    "search",
    "run",
    "run_tests",
    "install_packages",
    "todo_write",
    "code_engine",
    "tool_package",
    "deliver_file",
    "artifact_create",
    "artifact_update",
    "artifact_attach_data",
    "artifact_share_request",
    "share_file",
    "checkpoint_diff",
    "checkpoint_restore",
    "checkpoint_list",
    "symbols",
    "definition",
    "references",
    "diagnostics",
    "outline",
];

/// True for a tool the harness runs itself, so the VM's own tools: the ones an engine's plaza MCP
/// server must not offer, since the engine has its own through ACP.
pub(crate) fn is_guest_tool(name: &str) -> bool {
    GUEST_TOOLS.contains(&name) || tools::serves(name)
}

/// Runs a tool the host allowed, under the names the model knows.
pub(crate) fn run_guest_tool(
    workspace: &Workspace,
    work: u64,
    name: &str,
    input: &Value,
) -> Outcome {
    match name {
        "read" | "edit" | "find" | "search" | "run" | "run_tests" => {
            v2::serve(workspace, work, name, input).map_err(|err| err.to_string())
        }
        "install_packages" => bash::install_packages(workspace, input),
        "todo_write" => plan::todo_write(workspace, input),
        "tool_package" => library::package(workspace, input),
        "deliver_file" => library::deliver(workspace, input),
        "share_file" => library::share(workspace, input),
        "artifact_create" => library::artifact_create(workspace, input),
        "artifact_update" => library::artifact_update(workspace, input),
        "artifact_attach_data" => library::artifact_attach_data(workspace, input),
        "artifact_share_request" => library::artifact_share_request(workspace, input),
        "checkpoint_diff" => checkpoints::diff(workspace, input),
        "checkpoint_restore" => checkpoints::restore(workspace, input),
        "checkpoint_list" => checkpoints::list(workspace, input),
        "symbols" | "definition" | "references" | "diagnostics" | "outline" => {
            crate::code_intel::run(workspace, name, input)
        }
        // The owner's IDE tools come from the hub's operator, never from the model's own calls.
        name if name.starts_with("ide_") => Err(format!(
            "denied: {name} is an owner tool; the model is not offered it"
        )),
        _ => tools::serve(workspace, name, input).map_err(|err| err.to_string()),
    }
}

/// The size of a conversation as it is sent, in bytes.
fn serialized_len(messages: &[Message]) -> usize {
    serde_json::to_vec(messages).map_or(0, |bytes| bytes.len())
}

/// The head of the one summary block a compressed conversation carries in its first message.
const SUMMARY_HEAD: &str = "Summary of the work so far (earlier turns were compressed):";
/// The most a tool result kept after a compression may hold, except in the latest turn.
const KEPT_RESULT_BYTES: usize = 8 * 1024;

/// Where a compression cuts the conversation: an assistant message from which on the turns
/// stay verbatim. It starts at the `keep_recent`-th most recent assistant message (at least
/// the latest) and moves later while the kept tail is larger than `budget` bytes, so a
/// compression always brings the conversation well under its threshold. None when there is
/// no such message (fewer assistant messages than `keep_recent`).
fn compress_cut(messages: &[Message], keep_recent: u32, budget: usize) -> Option<usize> {
    let keep = keep_recent.max(1) as usize;
    let assistants: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == Role::Assistant)
        .map(|(index, _)| index)
        .collect();
    let last = *assistants.last()?;
    let mut position = assistants.len().checked_sub(keep)?;
    while assistants[position] < last && serialized_len(&messages[assistants[position]..]) > budget
    {
        position += 1;
    }
    Some(assistants[position])
}

/// Clips every tool result before the latest turn to KEPT_RESULT_BYTES: what was read long
/// ago is in the summary; the latest results stay whole.
fn clip_old_results(messages: &mut [Message]) {
    let latest = messages
        .iter()
        .rposition(|message| message.role == Role::Assistant)
        .unwrap_or(0);
    for message in &mut messages[..latest] {
        for block in &mut message.content {
            if let Block::ToolResult { content, .. } = block
                && content.len() > KEPT_RESULT_BYTES
            {
                *content = cap_output(content, KEPT_RESULT_BYTES);
            }
        }
    }
}

/// The file, under the workspace's state directory, that holds the operator's
/// chat threads across restarts.
const THREADS_FILE: &str = "threads.json";
/// The most a saved thread file may hold; the oldest threads are dropped first.
const THREADS_MAX_BYTES: usize = 1024 * 1024;

fn thread_json(thread: &Thread) -> Value {
    json!({
        "messages": serde_json::to_value(&thread.messages).unwrap_or(Value::Null),
        "summary": thread.summary,
        "at": thread.at,
    })
}

fn thread_from_json(value: &Value) -> Option<Thread> {
    Some(Thread {
        messages: serde_json::from_value(value.get("messages")?.clone()).ok()?,
        summary: value.get("summary")?.as_str()?.to_string(),
        at: value.get("at")?.as_u64()?,
    })
}

/// The operator's chat threads a previous session saved; none when the file is
/// absent or unreadable.
fn load_threads(workspace: &Workspace) -> HashMap<String, Thread> {
    let Ok(bytes) = std::fs::read(workspace.plaza_dir().join(THREADS_FILE)) else {
        return HashMap::new();
    };
    let Ok(Value::Object(saved)) = serde_json::from_slice::<Value>(&bytes) else {
        return HashMap::new();
    };
    saved
        .iter()
        .filter(|(id, _)| id.starts_with("op-"))
        .filter_map(|(id, value)| Some((id.clone(), thread_from_json(value)?)))
        .collect()
}

/// Saves the operator's chat threads (the `op-*` ones) by writing a temp file
/// and renaming it over the old one. Past [`THREADS_MAX_BYTES`] the oldest
/// threads are dropped first. Skips silently when the workspace is not mounted.
fn save_threads(workspace: &Workspace, threads: &HashMap<String, Thread>) -> std::io::Result<()> {
    if !workspace.dir().is_dir() {
        return Ok(());
    }
    // Newest first, so the oldest is last and goes first.
    let mut kept: Vec<(&String, &Thread)> = threads
        .iter()
        .filter(|(id, _)| id.starts_with("op-"))
        .collect();
    kept.sort_by_key(|a| std::cmp::Reverse(a.1.at));
    let render = |kept: &[(&String, &Thread)]| {
        let object: serde_json::Map<String, Value> = kept
            .iter()
            .map(|(id, thread)| ((*id).clone(), thread_json(thread)))
            .collect();
        serde_json::to_vec(&object)
    };
    let mut bytes = render(&kept)?;
    while bytes.len() > THREADS_MAX_BYTES && !kept.is_empty() {
        kept.pop();
        bytes = render(&kept)?;
    }

    let dir = workspace.plaza_dir();
    std::fs::create_dir_all(&dir)?;
    let temp = dir.join(format!("{THREADS_FILE}.tmp"));
    std::fs::write(&temp, &bytes)?;
    std::fs::rename(&temp, dir.join(THREADS_FILE))
}

#[cfg(test)]
mod thread_store_tests {
    use super::*;

    #[test]
    fn saved_threads_load_back_after_a_restart() {
        let dir = std::env::temp_dir().join(format!("apz-harness-threads-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch workspace");
        let workspace = Workspace::at(&dir);

        let mut threads = HashMap::new();
        threads.insert(
            "op-1".to_string(),
            Thread {
                messages: vec![Message::user_text("hello".to_string())],
                summary: "greeted".into(),
                at: 1,
            },
        );
        // A non-operator thread is not kept across restarts.
        threads.insert(
            "job-9".to_string(),
            Thread {
                messages: Vec::new(),
                summary: String::new(),
                at: 2,
            },
        );
        save_threads(&workspace, &threads).expect("threads save");

        let loaded = load_threads(&workspace);
        assert_eq!(loaded.len(), 1);
        let thread = &loaded["op-1"];
        assert_eq!(thread.messages, threads["op-1"].messages);
        assert_eq!(thread.summary, "greeted");
        assert_eq!(thread.at, 1);
        assert!(!dir.join(format!("{THREADS_FILE}.tmp")).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The start of a model result's error that means "held, ask again later" (the hub's budget, a limit, STOP ALL).
pub const PAUSED: &str = apz_proto::wire::MODEL_PAUSED;

/// How long to wait before asking again after the `n`-th "paused" answer in a row: 2, 4, 8, 15, then 30 seconds.
pub(crate) fn paused_wait(n: u32) -> Duration {
    Duration::from_secs(match n {
        0 => 2,
        1 => 4,
        2 => 8,
        3 => 15,
        _ => 30,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keepalive;
    use crate::session::Identity;
    use crate::session::testing::{next, pair};
    use apz_proto::line::Lines;
    use std::os::unix::net::UnixStream;

    const IDENTITY: Identity = Identity {
        guest_cid: 104,
        vm: 4,
    };
    const HEARTBEAT: Duration = Duration::from_millis(20);

    fn scratch(name: &str) -> Workspace {
        let dir =
            std::env::temp_dir().join(format!("apz-harness-agent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch workspace");
        Workspace::at(dir)
    }

    #[test]
    fn the_owners_ide_tools_are_refused_when_the_model_asks_for_them() {
        let workspace = scratch("ide-guard");
        let err = run_guest_tool(
            &workspace,
            1,
            "ide_write",
            &json!({"path": "a.txt", "text": "x", "expected_hash": null}),
        )
        .expect_err("the model may not write through the owner's tools");
        assert!(err.starts_with("denied: ide_write"), "{err}");
        assert!(!workspace.dir().join("a.txt").exists());
    }

    fn welcome(max_turns: u32) -> String {
        json!({
            "type": "welcome", "echo": "plaza-orchestrator", "role": "builder", "keepalive": true,
            "profile": {
                "name": "ada", "role": "builder", "model": "anthropic/x", "system_parts": [{"text": "be brief", "cache": true}],
                "tools": [{"name": "run", "description": "run", "input_schema": {"type": "object"}}],
                "max_turns": max_turns,
            },
        })
        .to_string()
    }

    /// The tools of a profile that defers one: `bash` in full, `ledger_rows` by name, and `tool_search`.
    fn deferred_tools() -> Value {
        json!([
            {"name": "run", "description": "run", "input_schema": {"type": "object"}},
            {"name": "ledger_rows", "description": "read the ledger rows", "deferred": true,
             "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}},
            {"name": "tool_search", "description": "fetch the schemas of deferred tools", "input_schema": {"type": "object"}},
        ])
    }

    /// A welcome whose profile offers `tools`, with the system prompt of [`welcome`].
    fn welcome_with_tools(tools: Value) -> String {
        json!({
            "type": "welcome", "echo": "plaza-orchestrator", "role": "builder", "keepalive": true,
            "profile": {
                "name": "ada", "role": "builder", "model": "anthropic/x",
                "system_parts": [{"text": "be brief", "cache": true}],
                "tools": tools, "max_turns": 10,
            },
        })
        .to_string()
    }

    fn work(id: u64, kind: &str, thread: &str, prompt: &str) -> String {
        json!({"type": "work", "work": id, "kind": kind, "thread": thread, "prompt": prompt})
            .to_string()
    }

    fn answer(req: u64, content: Value, stop: &str) -> String {
        answer_using(req, content, stop, 1)
    }

    /// The answer to model call `req`, whose context used `input` tokens.
    fn answer_using(req: u64, content: Value, stop: &str, input: u64) -> String {
        json!({
            "type": "model_result", "req": req, "ok": true,
            "response": {"model": "anthropic/x", "content": content, "stop": stop,
                         "usage": {"input_tokens": input, "output_tokens": 1}},
        })
        .to_string()
    }

    fn text(text: &str) -> Value {
        json!({"type": "text", "text": text})
    }

    fn tool_use(id: &str, name: &str, input: Value) -> Value {
        json!({"type": "tool_use", "id": id, "name": name, "input": input})
    }

    /// One call's decision, as an entry of a `tool_batch_decision`.
    fn entry(call: &str, decision: &str, output: &str, is_error: bool) -> Value {
        json!({"call": call, "decision": decision, "output": output, "is_error": is_error})
    }

    /// The hub's answer to the `tool_batch` of work `work`: `entries` in the calls' order.
    fn batch_answer(work: u64, entries: Vec<Value>) -> String {
        json!({"type": "tool_batch_decision", "work": work, "decisions": entries}).to_string()
    }

    /// The answer to a `tool_batch` of one call, for work 1 (the work item these tests run).
    fn decision(call: &str, decision: &str, output: &str, is_error: bool) -> String {
        batch_answer(1, vec![entry(call, decision, output, is_error)])
    }

    /// The hub's policy, as the line it pushes: the tools the harness may run without asking. The signature is the
    /// hub's; the harness keeps it as it came.
    fn policy_line(id: &str, allow: &[&str]) -> String {
        json!({"type": "policy", "policy": {"id": id, "issued_ms": 1, "allow": allow, "signature": "sig"}})
            .to_string()
    }

    /// The guest's next line that is not a heartbeat, parsed.
    fn next_msg(host: &mut Lines<UnixStream>) -> Value {
        loop {
            let value: Value = serde_json::from_str(&next(host)).expect("a JSON line");
            if value["type"] != "heartbeat" {
                return value;
            }
        }
    }

    fn expect_type(host: &mut Lines<UnixStream>, kind: &str) -> Value {
        let value = next_msg(host);
        assert_eq!(value["type"], kind, "{value}");
        value
    }

    /// The next line that is not the hub's answer to a memory query: a read or an edit asks the hub which entries are
    /// anchored to the paths it touched, after it ran. The query is answered with no entries, so the result stays as it was.
    fn next_past_memory(host: &mut Lines<UnixStream>) -> Value {
        loop {
            let value = next_msg(host);
            if value["type"] == "tool_request" && value["name"] == "memory_anchored" {
                let call = value["call"].as_str().expect("a call id").to_string();
                host.send(&json!({"type": "tool_decision", "call": call, "decision": "allow", "output": "{\"entries\":[]}"}).to_string())
                    .unwrap();
                continue;
            }
            return value;
        }
    }

    /// Expects a read's or an edit's `tool_report`, past the memory queries it made.
    fn expect_report(host: &mut Lines<UnixStream>) -> Value {
        let value = next_past_memory(host);
        assert_eq!(value["type"], "tool_report", "{value}");
        value
    }

    /// Ends the session the way the hub does, and checks the `bye`.
    fn stop(host: &mut Lines<UnixStream>) {
        host.send(r#"{"type":"stop","reason":"test over"}"#)
            .unwrap();
        expect_type(host, "bye");
    }

    /// Runs a keepalive session whose hub is `script`, and returns the calls served.
    fn run_hub(
        workspace: &Workspace,
        script: impl FnOnce(&mut Lines<UnixStream>) + Send + 'static,
    ) -> u64 {
        let (mut guest, mut host) = pair();
        let hub = thread::spawn(move || {
            next(&mut host);
            script(&mut host);
        });
        let mut log = Vec::new();
        let result = keepalive::session(
            &mut guest,
            &mut log,
            &IDENTITY,
            workspace,
            Some("builder"),
            None,
            HEARTBEAT,
        );
        drop(guest);
        hub.join().expect("the scripted hub finished");
        result.expect("the hub stops the agent")
    }

    #[test]
    fn tool_decisions_reach_the_next_model_call() {
        // An allowed tool runs here, and its output reaches the next model call.
        {
            let workspace = scratch("allow");
            run_hub(&workspace, |host| {
                host.send(&welcome(10)).unwrap();
                host.send(&work(1, "task", "t", "say hi")).unwrap();
                let first = expect_type(host, "model_call");
                assert_eq!(
                    (
                        first["req"].clone(),
                        first["request"]["system_parts"][0]["text"].clone()
                    ),
                    (json!(1), json!("be brief"))
                );
                assert_eq!(
                    first["request"]["messages"][0]["content"][0]["text"],
                    "say hi"
                );
                host.send(&answer(
                    1,
                    json!([tool_use("t1", "run", json!({"command": "echo hi"}))]),
                    "tool_use",
                ))
                .unwrap();
                let batch = expect_type(host, "tool_batch");
                assert_eq!(
                    (
                        batch["calls"][0]["call"].clone(),
                        batch["calls"][0]["name"].clone()
                    ),
                    (json!("t1"), json!("run"))
                );
                host.send(&decision("t1", "allow", "", false)).unwrap();
                let report = expect_type(host, "tool_report");
                assert_eq!(
                    (report["ok"].clone(), report["output"].clone()),
                    (json!(true), json!(r#"{"exit":0,"output":"hi\n"}"#))
                );
                let second = expect_type(host, "model_call");
                assert_eq!(
                    second["request"]["messages"][2]["content"][0]["content"],
                    r#"{"exit":0,"output":"hi\n"}"#
                );
                host.send(&answer(2, json!([text("all done")]), "end_turn"))
                    .unwrap();
                let done = expect_type(host, "work_done");
                assert_eq!(
                    (
                        done["work"].clone(),
                        done["status"].clone(),
                        done["summary"].clone()
                    ),
                    (json!(1), json!("done"), json!("all done"))
                );
                stop(host);
            });
        }
        // A host-run tool uses the decision's output, and the guest reports nothing.
        {
            let workspace = scratch("done");
            run_hub(&workspace, |host| {
                host.send(&welcome(10)).unwrap();
                host.send(&work(1, "task", "t", "find it")).unwrap();
                expect_type(host, "model_call");
                host.send(&answer(
                    1,
                    json!([tool_use("s1", "search", json!({"pattern": "x"}))]),
                    "tool_use",
                ))
                .unwrap();
                expect_type(host, "tool_batch");
                host.send(&decision("s1", "done", "3 hits", false)).unwrap();
                let next = expect_type(host, "model_call");
                assert_eq!(
                    next["request"]["messages"][2]["content"][0]["content"],
                    "3 hits"
                );
                host.send(&answer(2, json!([text("found")]), "end_turn"))
                    .unwrap();
                expect_type(host, "work_done");
                stop(host);
            });
        }
        // A denied tool is an error result carrying the reason.
        {
            let workspace = scratch("deny");
            run_hub(&workspace, |host| {
                host.send(&welcome(10)).unwrap();
                host.send(&work(1, "task", "t", "rm it")).unwrap();
                expect_type(host, "model_call");
                host.send(&answer(
                    1,
                    json!([tool_use("d1", "run", json!({"command": "rm -rf /"}))]),
                    "tool_use",
                ))
                .unwrap();
                expect_type(host, "tool_batch");
                host.send(&decision("d1", "deny", "not allowed here", true))
                    .unwrap();
                let next = expect_type(host, "model_call");
                let result = &next["request"]["messages"][2]["content"][0];
                assert_eq!(
                    (result["content"].clone(), result["is_error"].clone()),
                    (json!("not allowed here"), json!(true))
                );
                host.send(&answer(2, json!([text("ok")]), "end_turn"))
                    .unwrap();
                expect_type(host, "work_done");
                stop(host);
            });
        }
    }

    #[test]
    fn a_turn_of_five_calls_makes_one_tool_batch_for_the_hub() {
        let workspace = scratch("five");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(1, "task", "t", "five things")).unwrap();
            expect_type(host, "model_call");
            let calls: Vec<Value> = (1..=5)
                .map(|n| {
                    tool_use(
                        &format!("f{n}"),
                        "run",
                        json!({"command": format!("echo {n}")}),
                    )
                })
                .collect();
            host.send(&answer(1, json!(calls), "tool_use")).unwrap();
            // One round trip for the turn: all five calls in one batch, in the model's order.
            let batch = expect_type(host, "tool_batch");
            let sent: Vec<Value> = batch["calls"]
                .as_array()
                .unwrap()
                .iter()
                .map(|call| call["call"].clone())
                .collect();
            let expected: Vec<Value> = (1..=5).map(|n| json!(format!("f{n}"))).collect();
            assert_eq!(sent, expected, "{batch}");
            let answers: Vec<Value> = (1..=5)
                .map(|n| entry(&format!("f{n}"), "allow", "", false))
                .collect();
            host.send(&batch_answer(1, answers)).unwrap();
            // Each allowed call runs here and is reported, in order; the hub's own calls carry no policy.
            for n in 1..=5 {
                let report = expect_type(host, "tool_report");
                assert_eq!(
                    (report["call"].clone(), report["output"].clone()),
                    (
                        json!(format!("f{n}")),
                        json!(format!(r#"{{"exit":0,"output":"{n}\n"}}"#))
                    ),
                    "{report}"
                );
                assert!(report.get("policy").is_none(), "{report}");
            }
            let next = expect_type(host, "model_call");
            assert_eq!(
                next["request"]["messages"][2]["content"]
                    .as_array()
                    .map(Vec::len),
                Some(5)
            );
            host.send(&answer(2, json!([text("done")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn a_turn_the_policy_allows_makes_no_round_trip() {
        let workspace = scratch("local");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&policy_line("p1", &["run"])).unwrap();
            host.send(&work(1, "task", "t", "five things")).unwrap();
            expect_type(host, "model_call");
            let calls: Vec<Value> = (1..=5)
                .map(|n| {
                    tool_use(
                        &format!("l{n}"),
                        "run",
                        json!({"command": format!("echo {n}")}),
                    )
                })
                .collect();
            host.send(&answer(1, json!(calls), "tool_use")).unwrap();
            // The policy allows all five: they run here with no batch sent first, and each report carries its
            // record and the policy it was decided under.
            for n in 1..=5 {
                let report = expect_type(host, "tool_report");
                assert_eq!(
                    (
                        report["call"].clone(),
                        report["output"].clone(),
                        report["policy"].clone(),
                        report["name"].clone(),
                        report["input"].clone(),
                    ),
                    (
                        json!(format!("l{n}")),
                        json!(format!(r#"{{"exit":0,"output":"{n}\n"}}"#)),
                        json!("p1"),
                        json!("run"),
                        json!({"command": format!("echo {n}")}),
                    ),
                    "{report}"
                );
            }
            expect_type(host, "model_call");
            host.send(&answer(2, json!([text("done")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn a_turn_split_between_the_policy_and_the_hub_sends_the_hub_one_batch() {
        let workspace = scratch("mixed");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&policy_line("p2", &["run"])).unwrap();
            host.send(&work(1, "task", "t", "mixed")).unwrap();
            expect_type(host, "model_call");
            host.send(&answer(
                1,
                json!([
                    tool_use("m1", "run", json!({"command": "echo 1"})),
                    tool_use("m2", "search", json!({"pattern": "x"})),
                    tool_use("m3", "run", json!({"command": "echo 3"})),
                ]),
                "tool_use",
            ))
            .unwrap();
            // Only the search call is the hub's to decide.
            let batch = expect_type(host, "tool_batch");
            assert_eq!(batch["calls"].as_array().map(Vec::len), Some(1), "{batch}");
            assert_eq!(batch["calls"][0]["call"], "m2");
            host.send(&batch_answer(1, vec![entry("m2", "done", "3 hits", false)]))
                .unwrap();
            // The two bash calls run after the answer, in order, each reported under the policy.
            for (call, output) in [
                ("m1", r#"{"exit":0,"output":"1\n"}"#),
                ("m3", r#"{"exit":0,"output":"3\n"}"#),
            ] {
                let report = expect_type(host, "tool_report");
                assert_eq!(
                    (
                        report["call"].clone(),
                        report["policy"].clone(),
                        report["output"].clone()
                    ),
                    (json!(call), json!("p2"), json!(output)),
                    "{report}"
                );
            }
            let next = expect_type(host, "model_call");
            let results = &next["request"]["messages"][2]["content"];
            let contents: Vec<Value> = results
                .as_array()
                .unwrap()
                .iter()
                .map(|result| result["content"].clone())
                .collect();
            assert_eq!(
                contents,
                vec![
                    json!(r#"{"exit":0,"output":"1\n"}"#),
                    json!("3 hits"),
                    json!(r#"{"exit":0,"output":"3\n"}"#)
                ],
                "{results}"
            );
            host.send(&answer(2, json!([text("done")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn an_empty_policy_holds_every_call_to_the_hub() {
        let workspace = scratch("held");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            // STOP ALL sends an empty snapshot: nothing runs here until the hub sends one that allows it.
            host.send(&policy_line("held", &[])).unwrap();
            host.send(&work(1, "task", "t", "held")).unwrap();
            expect_type(host, "model_call");
            host.send(&answer(
                1,
                json!([tool_use("h1", "run", json!({"command": "echo 1"}))]),
                "tool_use",
            ))
            .unwrap();
            let batch = expect_type(host, "tool_batch");
            assert_eq!(batch["calls"][0]["call"], "h1");
            host.send(&decision("h1", "allow", "", false)).unwrap();
            let report = expect_type(host, "tool_report");
            assert!(report.get("policy").is_none(), "{report}");
            expect_type(host, "model_call");
            host.send(&answer(2, json!([text("done")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn a_deferred_tool_is_fetched_through_tool_search_and_then_offered_in_full() {
        let workspace = scratch("tool-search");
        run_hub(&workspace, |host| {
            host.send(&welcome_with_tools(deferred_tools())).unwrap();
            host.send(&work(1, "task", "t", "find the rows")).unwrap();
            let first = expect_type(host, "model_call");
            // The index: the deferred tool by name with an empty schema, the others as they are.
            assert_eq!(first["request"]["tools"][1]["name"], "ledger_rows");
            assert_eq!(
                first["request"]["tools"][1]["input_schema"],
                json!({"type": "object"})
            );
            assert_eq!(first["request"]["cache_last_message"], true);
            host.send(&answer(
                1,
                json!([tool_use("s1", "tool_search", json!({"query": "ledger"}))]),
                "tool_use",
            ))
            .unwrap();
            // The harness answers the search itself: no tool batch goes to the hub.
            let second = expect_type(host, "model_call");
            let result = &second["request"]["messages"][2]["content"][0];
            assert_eq!(result["is_error"], false, "{second}");
            assert!(
                result["content"]
                    .as_str()
                    .is_some_and(|text| text.contains(r#""path""#)),
                "{second}"
            );
            // From now on the schema is in the request.
            assert_eq!(
                second["request"]["tools"][1]["input_schema"]["properties"]["path"]["type"],
                "string"
            );
            assert!(second["request"]["tools"][1].get("deferred").is_none());
            host.send(&answer(2, json!([text("found")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn a_deferred_tool_called_before_its_fetch_is_refused_here() {
        let workspace = scratch("tool-search-refused");
        run_hub(&workspace, |host| {
            host.send(&welcome_with_tools(deferred_tools())).unwrap();
            host.send(&work(1, "task", "t", "read the rows")).unwrap();
            expect_type(host, "model_call");
            host.send(&answer(
                1,
                json!([tool_use("d1", "ledger_rows", json!({"path": "rows.csv"}))]),
                "tool_use",
            ))
            .unwrap();
            // Refused without a batch to the hub, and the model reads why.
            let second = expect_type(host, "model_call");
            let result = &second["request"]["messages"][2]["content"][0];
            assert_eq!(result["is_error"], true, "{second}");
            assert!(
                result["content"]
                    .as_str()
                    .is_some_and(|text| text.contains("fetch its schema with tool_search first")),
                "{second}"
            );
            host.send(&answer(2, json!([text("gave up")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn work_that_arrives_before_the_profile_waits_for_it_and_then_runs() {
        let workspace = scratch("profile-wait");
        run_hub(&workspace, |host| {
            // A hub restart can welcome the agent before its profile is ready.
            let mut bare: Value = serde_json::from_str(&welcome(10)).expect("a welcome");
            let profile = bare
                .as_object_mut()
                .expect("a welcome object")
                .remove("profile")
                .expect("a profile");
            host.send(&bare.to_string()).unwrap();
            host.send(&work(1, "task", "t", "say hi")).unwrap();
            // The work is held, not failed: the next line is a heartbeat, not its result.
            let early: Value = serde_json::from_str(&next(host)).expect("a JSON line");
            assert_eq!(early["type"], "heartbeat", "{early}");
            thread::sleep(Duration::from_millis(100));
            host.send(&json!({"type": "profile", "profile": profile}).to_string())
                .unwrap();
            let call = expect_type(host, "model_call");
            assert_eq!(
                call["request"]["system_parts"][0]["text"], "be brief",
                "{call}"
            );
            host.send(&answer(1, json!([text("hi back")]), "end_turn"))
                .unwrap();
            let done = expect_type(host, "work_done");
            assert_eq!(
                (done["work"].clone(), done["status"].clone()),
                (json!(1), json!("done"))
            );
            stop(host);
        });
    }

    fn control(work: u64, body: Value) -> String {
        json!({"type": "control", "work": work, "control": body}).to_string()
    }

    /// A profile that compresses whenever there is something older to fold.
    fn compressing_welcome() -> String {
        json!({
            "type": "welcome", "echo": "plaza-orchestrator", "role": "builder", "keepalive": true,
            "profile": {
                "name": "ada", "role": "builder", "model": "anthropic/x", "system_parts": [{"text": "be brief", "cache": true}],
                "tools": [{"name": "search", "description": "find", "input_schema": {"type": "object"}}],
                "max_turns": 10, "compress_at": 1, "keep_recent": 1,
            },
        })
        .to_string()
    }

    #[test]
    fn a_steer_control_reaches_the_next_model_call_as_a_note() {
        let workspace = scratch("steer");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(1, "task", "t", "go")).unwrap();
            expect_type(host, "model_call");
            host.send(&control(
                1,
                json!({"action": "steer", "from": "orchestrator", "text": "focus on the parser"}),
            ))
            .unwrap();
            host.send(&answer(
                1,
                json!([tool_use("s1", "search", json!({"pattern": "x"}))]),
                "tool_use",
            ))
            .unwrap();
            expect_type(host, "tool_batch");
            host.send(&decision("s1", "done", "3 hits", false)).unwrap();
            let next = expect_type(host, "model_call");
            let results = &next["request"]["messages"][2]["content"];
            assert_eq!(
                results[1]["text"], "[note from orchestrator]: focus on the parser",
                "{results}"
            );
            host.send(&answer(2, json!([text("found")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    fn step_welcome(max_turns: u32) -> String {
        json!({
            "type": "welcome", "echo": "plaza-orchestrator", "role": "builder", "keepalive": true,
            "profile": {
                "name": "ada", "role": "builder", "model": "anthropic/x", "system": "be brief",
                "tools": [{"name": "handover_write", "description": "hand over", "input_schema": {"type": "object"}}],
                "max_turns": max_turns, "handover": true,
            },
        })
        .to_string()
    }

    #[test]
    fn a_step_that_ends_without_a_handover_is_asked_for_one_twice_and_then_ends() {
        let workspace = scratch("handover-asks");
        run_hub(&workspace, |host| {
            host.send(&step_welcome(10)).unwrap();
            host.send(&work(1, "task", "t", "go")).unwrap();
            expect_type(host, "model_call");
            host.send(&answer(1, json!([text("done")]), "end_turn"))
                .unwrap();
            let first = expect_type(host, "model_call");
            let messages = &first["request"]["messages"];
            assert_eq!(messages.as_array().map(Vec::len), Some(3), "{messages}");
            assert!(
                messages[2]["content"][0]["text"]
                    .as_str()
                    .is_some_and(|text| text.starts_with("Write your handover with handover_write")),
                "{messages}"
            );
            host.send(&answer(2, json!([text("still done")]), "end_turn"))
                .unwrap();
            expect_type(host, "model_call");
            host.send(&answer(3, json!([text("done again")]), "end_turn"))
                .unwrap();
            let done = expect_type(host, "work_done");
            assert_eq!(done["status"], "done", "{done}");
            assert_eq!(done["summary"], "done again", "{done}");
            stop(host);
        });
    }

    #[test]
    fn a_step_ends_at_once_once_it_wrote_its_handover_or_says_the_task_is_done() {
        let workspace = scratch("handover-written");
        run_hub(&workspace, |host| {
            host.send(&step_welcome(10)).unwrap();
            host.send(&work(1, "task", "t", "go")).unwrap();
            expect_type(host, "model_call");
            host.send(&answer(
                1,
                json!([tool_use(
                    "h1",
                    "handover_write",
                    json!({"stands": "s", "done": "d", "next": "n"})
                )]),
                "tool_use",
            ))
            .unwrap();
            expect_type(host, "tool_batch");
            host.send(&decision("h1", "done", "handover recorded", false))
                .unwrap();
            expect_type(host, "model_call");
            host.send(&answer(2, json!([text("written")]), "end_turn"))
                .unwrap();
            let done = expect_type(host, "work_done");
            assert_eq!(done["status"], "done", "{done}");
            host.send(&work(2, "task", "t", "again")).unwrap();
            expect_type(host, "model_call");
            host.send(&answer(3, json!([text("TASK DONE")]), "end_turn"))
                .unwrap();
            let done = expect_type(host, "work_done");
            assert_eq!(done["summary"], "TASK DONE", "{done}");
            stop(host);
        });
    }

    #[test]
    fn a_long_conversation_is_compressed_before_the_next_model_call() {
        let workspace = scratch("compress");
        run_hub(&workspace, |host| {
            host.send(&compressing_welcome()).unwrap();
            host.send(&work(1, "task", "t", "go")).unwrap();
            for (req, id) in [(1, "s1"), (2, "s2")] {
                expect_type(host, "model_call");
                host.send(&answer(
                    req,
                    json!([tool_use(id, "search", json!({"pattern": id}))]),
                    "tool_use",
                ))
                .unwrap();
                expect_type(host, "tool_batch");
                host.send(&decision(id, "done", "hits", false)).unwrap();
            }
            let summary_call = expect_type(host, "model_call");
            let asked = &summary_call["request"]["messages"][0]["content"][0]["text"];
            assert!(
                asked
                    .as_str()
                    .is_some_and(|text| text.starts_with("Summarize")),
                "{asked}"
            );
            host.send(&answer(3, json!([text("the summary")]), "end_turn"))
                .unwrap();
            let compressed = expect_type(host, "compressed");
            assert_eq!(
                (
                    compressed["work"].clone(),
                    compressed["folded"].clone(),
                    compressed["reason"].clone()
                ),
                (json!(1), json!(2), json!("auto"))
            );
            let next = expect_type(host, "model_call");
            let messages = &next["request"]["messages"];
            assert_eq!(messages.as_array().map(Vec::len), Some(3), "{messages}");
            assert!(
                messages[0]["content"][1]["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("the summary"))
            );
            host.send(&answer(4, json!([text("done")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    /// A profile that compacts once its last call used 70 % of a 1000-token window; it has no byte threshold.
    fn windowed_welcome() -> String {
        json!({
            "type": "welcome", "echo": "plaza-orchestrator", "role": "builder", "keepalive": true,
            "profile": {
                "name": "ada", "role": "builder", "model": "anthropic/x", "system_parts": [{"text": "be brief", "cache": true}],
                "tools": [{"name": "search", "description": "find", "input_schema": {"type": "object"}}],
                "max_turns": 20, "compress_at": 0, "keep_recent": 1,
                "compact_at_pct": 70, "context_window": 1000,
            },
        })
        .to_string()
    }

    /// One turn of a windowed session: the call (request `req`) asks for the search `id`, which runs, and its
    /// context used 800 of the window's 1000 tokens.
    fn windowed_turn(host: &mut Lines<UnixStream>, req: u64, id: &str) {
        expect_type(host, "model_call");
        host.send(&answer_using(
            req,
            json!([tool_use(id, "search", json!({"pattern": id}))]),
            "tool_use",
            800,
        ))
        .unwrap();
        expect_type(host, "tool_batch");
        host.send(&decision(id, "done", "hits", false)).unwrap();
    }

    /// A compaction: the summary call (request `req`) is answered with `summary`. Returns the call and the
    /// `compressed` message.
    fn compaction(host: &mut Lines<UnixStream>, req: u64, summary: &str) -> (Value, Value) {
        let call = expect_type(host, "model_call");
        let asked = call["request"]["messages"][0]["content"][0]["text"].to_string();
        assert!(
            asked.contains("Summarize")
                && [
                    "Decisions",
                    "Dead ends",
                    "Open questions",
                    "Where it stands"
                ]
                .iter()
                .all(|part| asked.contains(part)),
            "{asked}"
        );
        host.send(&answer_using(req, json!([text(summary)]), "end_turn", 1))
            .unwrap();
        (call, expect_type(host, "compressed"))
    }

    #[test]
    fn a_context_under_the_share_of_the_window_is_not_compressed() {
        let workspace = scratch("under-share");
        run_hub(&workspace, |host| {
            host.send(&windowed_welcome()).unwrap();
            host.send(&work(1, "task", "t", "go")).unwrap();
            for (req, id) in [(1, "s1"), (2, "s2")] {
                expect_type(host, "model_call");
                host.send(&answer_using(
                    req,
                    json!([tool_use(id, "search", json!({"pattern": id}))]),
                    "tool_use",
                    600,
                ))
                .unwrap();
                expect_type(host, "tool_batch");
                host.send(&decision(id, "done", "hits", false)).unwrap();
            }
            // 600 of 1000 is under 70 %: the next call sees the whole conversation, with no summary asked.
            let next = expect_type(host, "model_call");
            assert_eq!(
                next["request"]["messages"].as_array().map(Vec::len),
                Some(5),
                "{next}"
            );
            host.send(&answer_using(3, json!([text("done")]), "end_turn", 600))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn a_decision_survives_three_compactions_of_the_context() {
        let workspace = scratch("three-compactions");
        run_hub(&workspace, |host| {
            host.send(&windowed_welcome()).unwrap();
            host.send(&work(1, "task", "t", "decide how agents are named"))
                .unwrap();
            // Two turns first: a compaction folds only what is older than the latest turn.
            windowed_turn(host, 1, "s1");
            windowed_turn(host, 2, "s2");
            let first = "Decisions: sessions, not named agents, because a seat is a session with its own history.\nDead ends: none.\nOpen questions: none.\nWhere it stands: the naming is settled.";
            let (_, compressed) = compaction(host, 3, first);
            assert_eq!(
                (compressed["folded"].clone(), compressed["reason"].clone()),
                (json!(2), json!("auto")),
                "{compressed}"
            );
            assert!(
                compressed["summary"]
                    .as_str()
                    .is_some_and(|summary| summary.contains("sessions, not named agents")),
                "{compressed}"
            );
            windowed_turn(host, 4, "s3");
            // The second summary is asked for with the first one in its transcript: the decision carries forward.
            let (second_call, _) = compaction(
                host,
                5,
                "Decisions: sessions, not named agents (a seat is a session).\nDead ends: none.\nOpen questions: none.\nWhere it stands: naming settled, two turns folded.",
            );
            assert!(
                second_call["request"]["messages"][0]["content"][0]["text"]
                    .to_string()
                    .contains("sessions, not named agents"),
                "{second_call}"
            );
            windowed_turn(host, 6, "s4");
            let (_, third) = compaction(
                host,
                7,
                "Decisions: sessions, not named agents (a seat is a session).\nDead ends: none.\nOpen questions: none.\nWhere it stands: naming settled, three compactions in.",
            );
            assert_eq!(third["folded"], json!(2), "{third}");
            // The call after the third compaction answers from the summary, not from the folded turns.
            let answering = expect_type(host, "model_call");
            let messages = &answering["request"]["messages"];
            assert_eq!(messages.as_array().map(Vec::len), Some(3), "{messages}");
            assert!(
                messages[0]
                    .to_string()
                    .contains("sessions, not named agents"),
                "{messages}"
            );
            host.send(&answer_using(
                8,
                json!([text("We decided on sessions, not named agents.")]),
                "end_turn",
                1,
            ))
            .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn malformed_tool_input_is_refused_without_asking_the_host() {
        let workspace = scratch("invalid");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(1, "task", "t", "go")).unwrap();
            expect_type(host, "model_call");
            let not_an_object = tool_use("i1", "run", json!("ls"));
            let gateway_meta = json!({"type": "tool_use", "id": "i2", "name": "run", "input": {},
                                      "meta": {"kind": "gateway", "error": "INVALID_JSON"}});
            host.send(&answer(1, json!([not_an_object, gateway_meta]), "tool_use"))
                .unwrap();
            let next = expect_type(host, "model_call");
            let results = &next["request"]["messages"][2]["content"];
            assert!(
                results[0]["content"]
                    .as_str()
                    .is_some_and(|c| c.starts_with("INVALID_INPUT: ")),
                "{results}"
            );
            assert!(
                results[1]["content"]
                    .as_str()
                    .is_some_and(|c| c.starts_with("INVALID_INPUT: ")),
                "{results}"
            );
            assert_eq!(results[1]["is_error"], true);
            host.send(&answer(2, json!([text("ok")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn the_loop_ends_the_work_on_refusal_token_limit_budget_or_a_stuck_call() {
        // A refusal fails the work item.
        {
            let workspace = scratch("refusal");
            run_hub(&workspace, |host| {
                host.send(&welcome(10)).unwrap();
                host.send(&work(1, "task", "t", "x")).unwrap();
                expect_type(host, "model_call");
                host.send(&answer(1, json!([text("no")]), "refusal"))
                    .unwrap();
                let done = expect_type(host, "work_done");
                assert_eq!(
                    (done["status"].clone(), done["summary"].clone()),
                    (json!("failed"), json!("the model declined"))
                );
                stop(host);
            });
        }
        // A second token limit stops the work after one continue.
        {
            let workspace = scratch("limit");
            run_hub(&workspace, |host| {
                host.send(&welcome(10)).unwrap();
                host.send(&work(1, "task", "t", "long")).unwrap();
                expect_type(host, "model_call");
                host.send(&answer(1, json!([text("partial")]), "max_tokens"))
                    .unwrap();
                let again = expect_type(host, "model_call");
                let messages = &again["request"]["messages"];
                assert_eq!(
                    messages[messages.as_array().unwrap().len() - 1]["content"][0]["text"],
                    "continue"
                );
                host.send(&answer(2, json!([text("more")]), "max_tokens"))
                    .unwrap();
                let done = expect_type(host, "work_done");
                assert_eq!(
                    (done["status"].clone(), done["summary"].clone()),
                    (json!("stopped"), json!("the model hit its token limit"))
                );
                stop(host);
            });
        }
        // The turn budget stops the work before another model call.
        {
            let workspace = scratch("budget");
            run_hub(&workspace, |host| {
                host.send(&welcome(2)).unwrap();
                host.send(&work(1, "task", "t", "loop")).unwrap();
                expect_type(host, "model_call");
                host.send(&answer(
                    1,
                    json!([tool_use("b1", "list", json!({"path": "a"}))]),
                    "tool_use",
                ))
                .unwrap();
                expect_type(host, "tool_batch");
                host.send(&decision("b1", "done", "[]", false)).unwrap();
                expect_type(host, "model_call");
                host.send(&answer(
                    2,
                    json!([tool_use("b2", "list", json!({"path": "b"}))]),
                    "tool_use",
                ))
                .unwrap();
                expect_type(host, "tool_batch");
                host.send(&decision("b2", "done", "[]", false)).unwrap();
                let done = expect_type(host, "work_done");
                assert_eq!(
                    (done["status"].clone(), done["summary"].clone()),
                    (json!("stopped"), json!("turn budget reached"))
                );
                stop(host);
            });
        }
        // The same failing call three times in a row is stuck.
        {
            let workspace = scratch("stuck");
            run_hub(&workspace, |host| {
                host.send(&welcome(10)).unwrap();
                host.send(&work(1, "task", "t", "again")).unwrap();
                for req in 1..=3 {
                    expect_type(host, "model_call");
                    let call = format!("s{req}");
                    host.send(&answer(
                        req,
                        json!([tool_use(&call, "run", json!({"command": "x"}))]),
                        "tool_use",
                    ))
                    .unwrap();
                    expect_type(host, "tool_batch");
                    host.send(&decision(&call, "deny", "no", true)).unwrap();
                }
                let done = expect_type(host, "work_done");
                assert_eq!(
                    (done["status"].clone(), done["summary"].clone()),
                    (
                        json!("stopped"),
                        json!("stuck: run failed 3 times in a row with the same input")
                    )
                );
                stop(host);
            });
        }
    }

    #[test]
    fn a_cancel_stops_the_work_before_its_next_model_call() {
        // A cancel stops after the current step and runs nothing more.
        {
            let workspace = scratch("cancel");
            run_hub(&workspace, |host| {
                host.send(&welcome(10)).unwrap();
                host.send(&work(1, "task", "t", "stop me")).unwrap();
                expect_type(host, "model_call");
                host.send(r#"{"type":"cancel","work":1,"reason":"operator stop"}"#)
                    .unwrap();
                host.send(&answer(
                    1,
                    json!([tool_use("c1", "run", json!({"command": "echo hi"}))]),
                    "tool_use",
                ))
                .unwrap();
                expect_type(host, "tool_batch");
                host.send(&decision("c1", "allow", "", false)).unwrap();
                expect_type(host, "tool_report");
                let done = expect_type(host, "work_done");
                assert_eq!(
                    (done["status"].clone(), done["summary"].clone()),
                    (json!("stopped"), json!("operator stop"))
                );
                stop(host);
            });
        }
        // A cancel that arrives while a tool runs stops the work before its next model call.
        {
            let workspace = scratch("cancel-in-tool");
            run_hub(&workspace, |host| {
                host.send(&welcome(10)).unwrap();
                host.send(&work(1, "task", "t", "run it")).unwrap();
                expect_type(host, "model_call");
                host.send(&answer(
                    1,
                    json!([tool_use("c1", "run", json!({"command": "sleep 1"}))]),
                    "tool_use",
                ))
                .unwrap();
                expect_type(host, "tool_batch");
                host.send(&decision("c1", "allow", "", false)).unwrap();
                // Sent while the tool runs: the loop takes it beside the tool, not after it.
                host.send(r#"{"type":"cancel","work":1,"reason":"operator stop"}"#)
                    .unwrap();
                expect_type(host, "tool_report");
                let done = expect_type(host, "work_done");
                assert_eq!(
                    (done["status"].clone(), done["summary"].clone()),
                    (json!("stopped"), json!("operator stop"))
                );
                stop(host);
            });
        }
    }

    #[test]
    fn a_chat_thread_keeps_its_conversation_verbatim_and_queued_work_runs_in_order() {
        let workspace = scratch("chat");
        let opaque = json!({"type": "opaque", "kind": "anthropic", "raw": {"type": "thinking", "signature": "sig"}});
        run_hub(&workspace, move |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(1, "chat", "room", "hello")).unwrap();
            expect_type(host, "model_call");
            host.send(&answer(
                1,
                json!([opaque.clone(), text("hi there")]),
                "end_turn",
            ))
            .unwrap();
            expect_type(host, "work_done");

            host.send(&work(2, "chat", "room", "again")).unwrap();
            let second = expect_type(host, "model_call");
            let messages = &second["request"]["messages"];
            assert_eq!(messages[0]["content"][0]["text"], "hello");
            assert_eq!(
                messages[1]["content"],
                json!([opaque.clone(), text("hi there")])
            );
            assert_eq!(messages[2]["content"][0]["text"], "again");
            // Work 3 arrives while work 2 waits for its model answer, and runs after it.
            host.send(&answer(2, json!([text("again!")]), "end_turn"))
                .unwrap();
            host.send(&work(3, "task", "other", "queued")).unwrap();
            expect_type(host, "work_done");
            let third = expect_type(host, "model_call");
            assert_eq!(
                (
                    third["work"].clone(),
                    third["request"]["messages"][0]["content"][0]["text"].clone()
                ),
                (json!(3), json!("queued"))
            );
            host.send(&answer(3, json!([text("queued done")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn heartbeats_go_out_while_a_model_call_is_pending() {
        let workspace = scratch("beats");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(1, "task", "t", "wait")).unwrap();
            expect_type(host, "model_call");
            let mut busy_beats = 0;
            while busy_beats < 3 {
                let value: Value = serde_json::from_str(&next(host)).expect("a JSON line");
                if value["type"] == "heartbeat" && value["busy"] == true {
                    busy_beats += 1;
                }
            }
            host.send(&answer(1, json!([text("late")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            let mut idle_beat = false;
            while !idle_beat {
                let value: Value = serde_json::from_str(&next(host)).expect("a JSON line");
                idle_beat = value["type"] == "heartbeat" && value["busy"] == false;
            }
            stop(host);
        });
    }

    #[test]
    fn a_paused_model_call_waits_and_asks_again_instead_of_failing() {
        let workspace = scratch("paused");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(1, "task", "t", "spend")).unwrap();
            expect_type(host, "model_call");
            let paused = json!({
                "type": "model_result", "req": 1, "ok": false,
                "error": "paused: task t1 reached its budget; the owner was asked",
            });
            host.send(&paused.to_string()).unwrap();
            // The same turn asks again (a new request id), and its answer ends the work as done.
            let again = expect_type(host, "model_call");
            assert_eq!(again["req"], 2, "{again}");
            host.send(&answer(
                2,
                json!([text("finished after the raise")]),
                "end_turn",
            ))
            .unwrap();
            let done = expect_type(host, "work_done");
            assert_eq!(done["status"], "done", "{done}");
            stop(host);
        });
    }

    #[test]
    fn a_chat_thread_past_its_size_limit_opens_a_new_conversation_with_its_summary() {
        let workspace = scratch("close");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(
                1,
                "chat",
                "long",
                &"x".repeat(THREAD_CLOSE_BYTES + 1),
            ))
            .unwrap();
            expect_type(host, "model_call");
            host.send(&answer(1, json!([text("summary one")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            host.send(&work(2, "chat", "long", "second")).unwrap();
            let next = expect_type(host, "model_call");
            let messages = next["request"]["messages"].as_array().unwrap();
            assert_eq!(messages.len(), 1, "{messages:?}");
            assert_eq!(
                messages[0]["content"][0]["text"],
                "Earlier in this conversation: summary one\n\nsecond"
            );
            host.send(&answer(2, json!([text("ok")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn an_operator_call_is_answered_while_the_agent_runs_a_local_tool() {
        let workspace = scratch("lane");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(1, "task", "t", "wait")).unwrap();
            expect_type(host, "model_call");
            host.send(&answer(
                1,
                json!([tool_use("slow", "run", json!({"command": "sleep 1"}))]),
                "tool_use",
            ))
            .unwrap();
            expect_type(host, "tool_batch");
            host.send(&decision("slow", "allow", "", false)).unwrap();
            // The local tool is sleeping now; the owner's call must not wait for it.
            host.send(r#"{"type":"tool_call","call":77,"name":"echo","args":{"text":"owner"}}"#)
                .unwrap();
            let first = next_msg(host);
            assert_eq!(
                (
                    first["type"].clone(),
                    first["call"].clone(),
                    first["output"].clone()
                ),
                (json!("tool_result"), json!(77), json!("owner")),
                "{first}"
            );
            expect_type(host, "tool_report");
            expect_type(host, "model_call");
            host.send(&answer(2, json!([text("done")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn operator_answers_and_heartbeats_never_interleave_on_the_wire() {
        let workspace = scratch("interleave");
        let padding = "x".repeat(400);
        run_hub(&workspace, move |host| {
            host.send(&welcome(10)).unwrap();
            for call in 1..=200u64 {
                let text = format!("{call}-{padding}");
                let line = json!({"type": "tool_call", "call": call, "name": "echo", "args": {"text": text}});
                host.send(&line.to_string()).unwrap();
            }
            // Every answer arrives whole and in order: a torn line would fail to parse in `next_msg`.
            for call in 1..=200u64 {
                let answer = next_msg(host);
                assert_eq!(
                    (
                        answer["type"].clone(),
                        answer["call"].clone(),
                        answer["ok"].clone()
                    ),
                    (json!("tool_result"), json!(call), json!(true)),
                    "{answer}"
                );
            }
            stop(host);
        });
    }

    #[test]
    fn the_v2_tools_are_served_here_and_the_v1_names_are_not() {
        let workspace = scratch("v2-served");
        std::fs::write(workspace.dir().join("a.txt"), "one\ntwo\n").expect("a scratch file");
        let out = run_guest_tool(
            &workspace,
            1,
            "read",
            &json!({"files": [{"path": "a.txt"}]}),
        )
        .expect("a read");
        let reply: Value = serde_json::from_str(&out).expect("JSON");
        assert_eq!(reply["files"][0]["lines"], 2, "{reply}");
        let err = run_guest_tool(&workspace, 1, "read_file", &json!({"path": "a.txt"}))
            .expect_err("read_file is gone");
        assert_eq!(err, "no such tool: read_file");
    }

    /// A `/proc/<pid>/stat` line as the kernel writes it, with a name that holds a space and a bracket.
    const STAT: &str = "4242 (my (worker) 2) S 1 4242 4242 0 -1 4194560 1200 0 0 0 730 270 0 0 20 0 3 0 99 0 0 0 0";

    /// The `VmRSS` line of a `/proc/<pid>/status` file, between the lines a real one has.
    const STATUS: &str = "Name:\tworker\nState:\tS (sleeping)\nVmPeak:\t  300000 kB\nVmRSS:\t  204800 kB\nThreads:\t3\n";

    #[test]
    fn a_stat_line_with_spaces_and_brackets_in_its_name_gives_the_name_and_the_cpu_ticks() {
        assert_eq!(parse_stat(STAT), Some(("my (worker) 2".to_string(), 1000)));
        assert_eq!(parse_stat("4242 (short) S 1"), None);
    }

    #[test]
    fn the_resident_memory_is_read_from_the_vm_rss_line_in_kib() {
        assert_eq!(parse_rss_kib(STATUS), Some(204_800));
        assert_eq!(parse_rss_kib("Name:\tkthreadd\nState:\tS\n"), None);
    }

    #[test]
    fn the_busiest_processes_rank_by_cpu_since_the_last_report() {
        let process = |pid: u32, name: &str, ticks: u64, rss_mib: f64| ProcSample {
            pid,
            name: name.to_string(),
            ticks,
            rss_mib,
        };
        let previous = HashMap::from([(1, 100), (2, 500), (3, 0)]);
        let now = [
            // 300 ticks over 60 s is 5 %.
            process(1, "idle", 400, 10.0),
            // 1500 ticks over 60 s is 25 %.
            process(2, "busy", 2000, 80.0),
            // A new process: all its 3000 ticks count, 50 %.
            process(4, "new", 3000, 5.0),
        ];
        let top = busiest(&previous, &now, 60.0, 2);
        assert_eq!(
            top,
            vec![
                ("new".to_string(), 50.0, 5.0),
                ("busy".to_string(), 25.0, 80.0),
            ]
        );
        assert!(
            busiest(&previous, &now, 0.0, 5)
                .iter()
                .all(|row| row.1 == 0.0)
        );
    }

    /// A named pipe in the workspace. A read of it waits until a writer opens and closes it, so a read takes as long as
    /// the writer keeps it waiting.
    fn pipe(workspace: &Workspace, name: &str) -> std::path::PathBuf {
        let path = workspace.dir().join(name);
        let made = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .expect("mkfifo runs");
        assert!(made.success(), "mkfifo {name}");
        path
    }

    /// One fragment of model call `req`'s answer, as the hub sends it while the answer streams.
    fn fragment(req: u64, delta: Value) -> String {
        json!({"type": "model_delta", "req": req, "delta": delta}).to_string()
    }

    #[test]
    fn a_read_shows_the_entries_anchored_to_its_file_once_per_work_item() {
        let workspace = scratch("anchored-read");
        std::fs::write(
            workspace.dir().join("placement.rs"),
            "fn move_claimed() {}\n",
        )
        .unwrap();
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&policy_line("reads", &["read"])).unwrap();
            host.send(&work(1, "task", "t", "read the placement"))
                .unwrap();
            expect_type(host, "model_call");
            let read =
                |call: &str| tool_use(call, "read", json!({"files": [{"path": "placement.rs"}]}));
            host.send(&answer(1, json!([read("r1")]), "tool_use"))
                .unwrap();
            // The first read asks the hub which entries are anchored to the file, and shows the one it answers with.
            let asked = expect_type(host, "tool_request");
            assert_eq!(asked["name"], "memory_anchored", "{asked}");
            assert_eq!(asked["input"]["paths"], json!(["placement.rs"]));
            let entry = json!({"id": "P-0017", "kind": "pitfall", "status": "active", "title": "A move releases its claim", "text": "Release it first."});
            host.send(&json!({"type": "tool_decision", "call": asked["call"], "decision": "allow", "output": json!({"entries": [entry]}).to_string()}).to_string()).unwrap();
            let report = expect_type(host, "tool_report");
            let output: Value = serde_json::from_str(report["output"].as_str().unwrap()).unwrap();
            assert_eq!(output["memory"][0]["id"], "P-0017", "{report}");
            // A second read of the file in the same work item asks no more, and shows nothing new.
            expect_type(host, "model_call");
            host.send(&answer(2, json!([read("r2")]), "tool_use"))
                .unwrap();
            let again = expect_type(host, "tool_report");
            let output: Value = serde_json::from_str(again["output"].as_str().unwrap()).unwrap();
            assert!(output.get("memory").is_none(), "{again}");
            expect_type(host, "model_call");
            stop(host);
        });
    }

    #[test]
    fn five_reads_of_different_files_finish_the_turn_in_about_one_read() {
        let workspace = scratch("five-reads");
        // Each file is a pipe that a writer fills 300 ms after a read opens it, so one read takes 300 ms.
        let names: Vec<String> = (1..=5).map(|n| format!("slow{n}.txt")).collect();
        for name in &names {
            let path = pipe(&workspace, name);
            thread::spawn(move || {
                let Ok(mut writer) = std::fs::OpenOptions::new().write(true).open(&path) else {
                    return;
                };
                thread::sleep(Duration::from_millis(300));
                let _ = writer.write_all(b"slow\n");
            });
        }
        run_hub(&workspace, move |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&policy_line("reads", &["read"])).unwrap();
            host.send(&work(1, "task", "t", "read five")).unwrap();
            expect_type(host, "model_call");
            let calls: Vec<Value> = names
                .iter()
                .enumerate()
                .map(|(n, name)| {
                    tool_use(&format!("r{n}"), "read", json!({"files": [{"path": name}]}))
                })
                .collect();
            let asked = Instant::now();
            host.send(&answer(1, json!(calls), "tool_use")).unwrap();
            // The reports go out in the answer's order, once all five reads are done.
            for n in 0..5 {
                let report = expect_report(host);
                assert_eq!(
                    (report["call"].clone(), report["ok"].clone()),
                    (json!(format!("r{n}")), json!(true)),
                    "{report}"
                );
            }
            expect_type(host, "model_call");
            let took = asked.elapsed();
            assert!(
                took < Duration::from_millis(900),
                "five reads took {took:?}; one read takes 300 ms"
            );
            host.send(&answer(2, json!([text("done")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn two_edits_of_one_file_keep_their_order() {
        let workspace = scratch("two-edits");
        let notes = workspace.dir().join("notes.txt");
        std::fs::write(&notes, "a").unwrap();
        run_hub(&workspace, move |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&policy_line("edits", &["edit"])).unwrap();
            host.send(&work(1, "task", "t", "edit twice")).unwrap();
            expect_type(host, "model_call");
            // The second edit finds "b" only once the first has written it.
            host.send(&answer(
                1,
                json!([
                    tool_use(
                        "e1",
                        "edit",
                        json!({"edits": [{"path": "notes.txt", "old": "a", "new": "b"}]})
                    ),
                    tool_use(
                        "e2",
                        "edit",
                        json!({"edits": [{"path": "notes.txt", "old": "b", "new": "c"}]})
                    ),
                ]),
                "tool_use",
            ))
            .unwrap();
            for call in ["e1", "e2"] {
                let report = expect_report(host);
                assert_eq!(
                    (report["call"].clone(), report["ok"].clone()),
                    (json!(call), json!(true)),
                    "{report}"
                );
            }
            expect_type(host, "model_call");
            assert_eq!(std::fs::read_to_string(&notes).unwrap(), "c");
            host.send(&answer(2, json!([text("done")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn a_read_whose_call_is_whole_starts_before_the_answer_ends_and_its_result_is_the_one_reported()
    {
        let workspace = scratch("ahead");
        let path = pipe(&workspace, "ahead.txt");
        // The writer opens the pipe, which only a running read does, and writes once the test says so.
        let (opened, reader_opened) = mpsc::channel();
        let (go, write_now) = mpsc::channel::<()>();
        thread::spawn(move || {
            let Ok(mut writer) = std::fs::OpenOptions::new().write(true).open(&path) else {
                return;
            };
            let _ = opened.send(());
            if write_now.recv().is_ok() {
                let _ = writer.write_all(b"early data\n");
            }
        });
        run_hub(&workspace, move |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&policy_line("ahead", &["read"])).unwrap();
            host.send(&work(1, "task", "t", "read ahead")).unwrap();
            expect_type(host, "model_call");
            // The answer streams the read as a whole call: its start, its input and its end.
            host.send(&fragment(
                1,
                json!({"type": "tool_use", "name": "read", "index": 0, "id": "r1"}),
            ))
            .unwrap();
            host.send(&fragment(
                1,
                json!({"type": "tool_input", "index": 0, "json": "{\"files\": [{\"path\": \"ahead.txt\"}]}"}),
            ))
            .unwrap();
            host.send(&fragment(1, json!({"type": "block_end", "index": 0})))
                .unwrap();
            // The read has started while the answer is still to come.
            reader_opened
                .recv_timeout(Duration::from_secs(5))
                .expect("the read started before the answer ended");
            host.send(&answer(
                1,
                json!([tool_use(
                    "r1",
                    "read",
                    json!({"files": [{"path": "ahead.txt"}]})
                )]),
                "tool_use",
            ))
            .unwrap();
            // The writer writes only now, so the read's result can only be the one it started with.
            go.send(()).unwrap();
            let report = expect_report(host);
            assert_eq!(report["call"], "r1", "{report}");
            assert!(
                report["output"]
                    .as_str()
                    .is_some_and(|output| output.contains("early data")),
                "{report}"
            );
            expect_type(host, "model_call");
            host.send(&answer(2, json!([text("done")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn a_write_never_starts_before_the_answer_ends() {
        let workspace = scratch("ahead-write");
        std::fs::write(workspace.dir().join("w.txt"), "a").unwrap();
        let file = workspace.dir().join("w.txt");
        run_hub(&workspace, move |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&policy_line("write", &["edit"])).unwrap();
            host.send(&work(1, "task", "t", "edit")).unwrap();
            expect_type(host, "model_call");
            host.send(&fragment(
                1,
                json!({"type": "tool_use", "name": "edit", "index": 0, "id": "w1"}),
            ))
            .unwrap();
            host.send(&fragment(1, json!({"type": "tool_input", "index": 0, "json": "{\"edits\": [{\"path\": \"w.txt\", \"old\": \"a\", \"new\": \"b\"}]}"})))
                .unwrap();
            host.send(&fragment(1, json!({"type": "block_end", "index": 0})))
                .unwrap();
            // A write that had started early would have landed by now.
            thread::sleep(Duration::from_millis(300));
            assert_eq!(std::fs::read_to_string(&file).unwrap(), "a");
            host.send(&answer(
                1,
                json!([tool_use(
                    "w1",
                    "edit",
                    json!({"edits": [{"path": "w.txt", "old": "a", "new": "b"}]})
                )]),
                "tool_use",
            ))
            .unwrap();
            let report = expect_report(host);
            assert_eq!(report["ok"], json!(true), "{report}");
            expect_type(host, "model_call");
            assert_eq!(std::fs::read_to_string(&file).unwrap(), "b");
            host.send(&answer(2, json!([text("done")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn a_read_the_policy_does_not_allow_never_starts_before_the_answer_ends() {
        let workspace = scratch("ahead-held");
        let path = pipe(&workspace, "held.txt");
        let (opened, reader_opened) = mpsc::channel();
        thread::spawn(move || {
            let Ok(mut writer) = std::fs::OpenOptions::new().write(true).open(&path) else {
                return;
            };
            let _ = opened.send(());
            let _ = writer.write_all(b"held\n");
        });
        run_hub(&workspace, move |host| {
            host.send(&welcome(10)).unwrap();
            // STOP ALL: an empty policy allows nothing, so the read goes to the hub with the rest of the answer.
            host.send(&policy_line("held", &[])).unwrap();
            host.send(&work(1, "task", "t", "read held")).unwrap();
            expect_type(host, "model_call");
            host.send(&fragment(
                1,
                json!({"type": "tool_use", "name": "read", "index": 0, "id": "h1"}),
            ))
            .unwrap();
            host.send(&fragment(
                1,
                json!({"type": "tool_input", "index": 0, "json": "{\"files\": [{\"path\": \"held.txt\"}]}"}),
            ))
            .unwrap();
            host.send(&fragment(1, json!({"type": "block_end", "index": 0})))
                .unwrap();
            // Nothing opens the pipe before the answer is in.
            assert!(
                reader_opened
                    .recv_timeout(Duration::from_millis(300))
                    .is_err(),
                "the read ran before the answer ended"
            );
            host.send(&answer(
                1,
                json!([tool_use(
                    "h1",
                    "read",
                    json!({"files": [{"path": "held.txt"}]})
                )]),
                "tool_use",
            ))
            .unwrap();
            let batch = expect_type(host, "tool_batch");
            assert_eq!(batch["calls"][0]["call"], "h1", "{batch}");
            host.send(&decision("h1", "allow", "", false)).unwrap();
            let report = expect_report(host);
            assert!(report.get("policy").is_none(), "{report}");
            expect_type(host, "model_call");
            host.send(&answer(2, json!([text("done")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn a_stats_report_reads_the_process_table_it_is_given() {
        let dir = std::env::temp_dir().join(format!("apz-proc-{}", std::process::id()));
        let worker = dir.join("4242");
        fs::create_dir_all(&worker).unwrap();
        fs::write(worker.join("stat"), STAT).unwrap();
        fs::write(worker.join("status"), STATUS).unwrap();
        fs::create_dir_all(dir.join("not-a-pid")).unwrap();

        let samples = sample_processes(&dir);
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            samples,
            vec![ProcSample {
                pid: 4242,
                name: "my (worker) 2".to_string(),
                ticks: 1000,
                rss_mib: 200.0,
            }]
        );
    }

    #[test]
    fn a_tool_report_carries_how_long_its_call_ran_and_a_timed_run_keeps_its_outcome() {
        let report = tool_report(1, "toolu_1", true, "ok".into(), None, 7, None);
        assert_eq!(
            report,
            GuestMessage::ToolReport {
                work: 1,
                call: "toolu_1".into(),
                ok: true,
                output: "ok".into(),
                policy: None,
                name: None,
                input: None,
                duration_ms: Some(7),
                script: None,
            }
        );
        let timed = Timed::run(|| {
            thread::sleep(Duration::from_millis(20));
            Err("late".to_string())
        });
        assert_eq!(timed.outcome, Err("late".to_string()));
        assert!(timed.ms >= 20, "{}", timed.ms);
    }

    /// A profile that offers the tools the script tests call: `read`, `run`, `tool_search`, and `run_script`.
    fn script_tools() -> Value {
        json!([
            {"name": "read", "description": "read files", "input_schema": {"type": "object", "properties": {"files": {"type": "array"}}}},
            {"name": "run", "description": "run a command", "input_schema": {"type": "object"}},
            {"name": "tool_search", "description": "fetch the schemas of deferred tools", "input_schema": {"type": "object"}},
            {"name": "run_script", "description": "run a script", "input_schema": {"type": "object"}},
        ])
    }

    /// The `run_script` call a model makes with `language` and `code`, and an optional time limit.
    fn script_use(id: &str, language: &str, code: &str, timeout_s: Option<u64>) -> Value {
        let mut input = json!({"language": language, "code": code});
        if let Some(seconds) = timeout_s {
            input["timeout_s"] = json!(seconds);
        }
        tool_use(id, "run_script", input)
    }

    /// The ten-file survey: read each file, keep the names of those with an error, and set the summary.
    const SURVEY: &str = "rows = []\nfor n in range(10):\n    text = read({\"files\": [{\"path\": \"f%d.txt\" % n}]})\n    if \"ERROR\" in text:\n        rows.append(\"f%d.txt\" % n)\nresult({\"files\": 10, \"with_errors\": rows})\n";

    #[test]
    fn a_ten_file_survey_is_one_model_call_and_its_reads_run_under_the_policy() {
        let workspace = scratch("survey");
        for n in 0..10 {
            let body = if n == 3 || n == 7 {
                "ERROR: bad row\n"
            } else {
                "fine\n"
            };
            std::fs::write(workspace.dir().join(format!("f{n}.txt")), body).unwrap();
        }
        run_hub(&workspace, |host| {
            host.send(&welcome_with_tools(script_tools())).unwrap();
            host.send(&policy_line("survey", &["read"])).unwrap();
            host.send(&work(1, "task", "t", "survey the files"))
                .unwrap();
            expect_type(host, "model_call");
            host.send(&answer(
                1,
                json!([script_use("s1", "python", SURVEY, None)]),
                "tool_use",
            ))
            .unwrap();
            // The script is not under the policy: the hub decides it, once.
            let batch = expect_type(host, "tool_batch");
            assert_eq!(batch["calls"][0]["name"], "run_script", "{batch}");
            host.send(&batch_answer(1, vec![entry("s1", "allow", "", false)]))
                .unwrap();
            // Its ten reads run here under the policy, each reported under the script call.
            for n in 0..10 {
                let report = expect_report(host);
                assert_eq!(report["call"], json!(format!("s1.{}", n + 1)), "{report}");
                assert_eq!(report["script"], "s1", "{report}");
                assert_eq!(report["policy"], "survey", "{report}");
                assert_eq!(report["name"], "read", "{report}");
            }
            // The script's own call is reported when it returns, under no policy: the hub allowed it.
            let outer = expect_report(host);
            assert_eq!(outer["call"], "s1", "{outer}");
            assert!(
                outer.get("policy").is_none() && outer.get("script").is_none(),
                "{outer}"
            );
            let value: Value = serde_json::from_str(outer["output"].as_str().unwrap()).unwrap();
            assert_eq!(
                value,
                json!({"files": 10, "with_errors": ["f3.txt", "f7.txt"]}),
                "{outer}"
            );
            // The model's next call sees the summary as the script call's result, and nothing of the ten reads.
            let next = expect_type(host, "model_call");
            let seen = next.to_string();
            assert!(seen.contains("with_errors"), "{seen}");
            assert!(!seen.contains("bad row"), "{seen}");
            host.send(&answer(2, json!([text("done")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
            stop(host);
        });
    }

    #[test]
    fn a_script_call_outside_the_policy_goes_to_the_hub_in_a_batch_of_its_own() {
        let workspace = scratch("script-hub");
        run_hub(&workspace, |host| {
            host.send(&welcome_with_tools(script_tools())).unwrap();
            host.send(&policy_line("reads", &["read"])).unwrap();
            host.send(&work(1, "task", "t", "run one command")).unwrap();
            expect_type(host, "model_call");
            let code = "result(run({\"command\": \"echo hi\"}))\n";
            host.send(&answer(
                1,
                json!([script_use("s2", "python", code, None)]),
                "tool_use",
            ))
            .unwrap();
            let outer = expect_type(host, "tool_batch");
            assert_eq!(outer["calls"][0]["name"], "run_script", "{outer}");
            host.send(&batch_answer(1, vec![entry("s2", "allow", "", false)]))
                .unwrap();
            // The script's `run` is not under the policy: it goes to the hub, alone, under its own call id.
            let inner = expect_type(host, "tool_batch");
            assert_eq!(inner["calls"].as_array().map(Vec::len), Some(1), "{inner}");
            assert_eq!(inner["calls"][0]["call"], "s2.1", "{inner}");
            assert_eq!(inner["calls"][0]["name"], "run", "{inner}");
            host.send(&batch_answer(1, vec![entry("s2.1", "allow", "", false)]))
                .unwrap();
            // The hub allowed it, so it runs here, reported under the script call.
            let report = expect_report(host);
            assert_eq!(report["call"], "s2.1", "{report}");
            assert_eq!(report["script"], "s2", "{report}");
            assert!(report.get("policy").is_none(), "{report}");
            let outer = expect_report(host);
            assert_eq!(outer["call"], "s2", "{outer}");
            assert!(outer["output"].as_str().unwrap().contains("hi"), "{outer}");
            expect_type(host, "model_call");
            stop(host);
        });
    }

    #[test]
    fn a_call_the_hub_denies_raises_in_the_script_and_its_message_comes_back() {
        let workspace = scratch("script-denied");
        run_hub(&workspace, |host| {
            host.send(&welcome_with_tools(script_tools())).unwrap();
            host.send(&policy_line("none", &[])).unwrap();
            host.send(&work(1, "task", "t", "try one command")).unwrap();
            expect_type(host, "model_call");
            let code = "try:\n    run({\"command\": \"rm -rf /\"})\nexcept PlazaError as err:\n    result(str(err))\n";
            host.send(&answer(
                1,
                json!([script_use("s3", "python", code, None)]),
                "tool_use",
            ))
            .unwrap();
            expect_type(host, "tool_batch");
            host.send(&batch_answer(1, vec![entry("s3", "allow", "", false)]))
                .unwrap();
            let inner = expect_type(host, "tool_batch");
            assert_eq!(inner["calls"][0]["call"], "s3.1", "{inner}");
            host.send(&batch_answer(
                1,
                vec![entry("s3.1", "deny", "the owner denied it", true)],
            ))
            .unwrap();
            // The denial is the script's to catch: its message is the script's value, and no report is made for it.
            let outer = expect_report(host);
            assert_eq!(outer["call"], "s3", "{outer}");
            assert_eq!(outer["output"], "the owner denied it", "{outer}");
            assert_eq!(outer["ok"], true, "{outer}");
            expect_type(host, "model_call");
            stop(host);
        });
    }

    #[test]
    fn a_script_past_its_time_is_stopped_and_its_call_says_so() {
        let workspace = scratch("script-time");
        run_hub(&workspace, |host| {
            host.send(&welcome_with_tools(script_tools())).unwrap();
            host.send(&policy_line("none", &[])).unwrap();
            host.send(&work(1, "task", "t", "wait")).unwrap();
            expect_type(host, "model_call");
            let code = "import time\ntime.sleep(60)\n";
            host.send(&answer(
                1,
                json!([script_use("s4", "python", code, Some(1))]),
                "tool_use",
            ))
            .unwrap();
            expect_type(host, "tool_batch");
            host.send(&batch_answer(1, vec![entry("s4", "allow", "", false)]))
                .unwrap();
            let started = Instant::now();
            let outer = expect_report(host);
            assert_eq!(outer["ok"], false, "{outer}");
            assert!(
                outer["output"]
                    .as_str()
                    .unwrap()
                    .starts_with("[timed out after 1 s; the script was killed]"),
                "{outer}"
            );
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "{:?}",
                started.elapsed()
            );
            expect_type(host, "model_call");
            stop(host);
        });
    }

    #[test]
    fn a_script_past_its_output_cap_is_stopped_and_its_call_says_so() {
        let workspace = scratch("script-output");
        run_hub(&workspace, |host| {
            host.send(&welcome_with_tools(script_tools())).unwrap();
            host.send(&policy_line("none", &[])).unwrap();
            host.send(&work(1, "task", "t", "print a lot")).unwrap();
            expect_type(host, "model_call");
            // 3000 lines of 101 bytes: more than the 256 KiB the script may print.
            let code = "for i in range(3000):\n    print('x' * 100)\n";
            host.send(&answer(
                1,
                json!([script_use("s5", "python", code, None)]),
                "tool_use",
            ))
            .unwrap();
            expect_type(host, "tool_batch");
            host.send(&batch_answer(1, vec![entry("s5", "allow", "", false)]))
                .unwrap();
            let outer = expect_report(host);
            assert_eq!(outer["ok"], false, "{outer}");
            assert!(
                outer["output"]
                    .as_str()
                    .unwrap()
                    .contains("printed more than 256 KiB"),
                "{outer}"
            );
            expect_type(host, "model_call");
            stop(host);
        });
    }

    #[test]
    fn a_script_makes_at_most_fifty_calls_and_the_rest_are_refused_in_the_script() {
        let workspace = scratch("script-calls");
        run_hub(&workspace, |host| {
            host.send(&welcome_with_tools(script_tools())).unwrap();
            host.send(&policy_line("none", &[])).unwrap();
            host.send(&work(1, "task", "t", "count")).unwrap();
            expect_type(host, "model_call");
            // Fifty-two refused calls (tool_search with no query is refused here, not by the hub): two are past the cap.
            let code = "over = 0\nfor n in range(52):\n    try:\n        call(\"tool_search\", {})\n    except PlazaError as err:\n        if \"at most 50\" in str(err):\n            over += 1\nresult(over)\n";
            host.send(&answer(
                1,
                json!([script_use("s6", "python", code, None)]),
                "tool_use",
            ))
            .unwrap();
            expect_type(host, "tool_batch");
            host.send(&batch_answer(1, vec![entry("s6", "allow", "", false)]))
                .unwrap();
            let outer = expect_report(host);
            assert_eq!(outer["output"], "2", "{outer}");
            expect_type(host, "model_call");
            stop(host);
        });
    }
}
