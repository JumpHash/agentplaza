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
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
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
use crate::files::{self, Workspace};
use crate::lane::{Duplex, Inbound, OPERATOR_EXPIRY, Shared, read_host, serve_operators};
use crate::session::{SessionError, escape_for_transcript, kind_of};
use crate::tools;
use crate::{checkpoints, library, plan, search_tools, shells};

/// A chat thread whose history serializes past this is closed: its next work
/// starts a new conversation, opened with the thread's last summary.
pub const THREAD_CLOSE_BYTES: usize = 400 * 1024;
/// Consecutive identical failing tool calls that stop a work item.
const STUCK_FAILURES: u32 = 3;
/// Token-limit stops a work item continues past (with a "continue" message).
const TOKEN_LIMIT_CONTINUES: u32 = 1;
/// How often a local tool's thread is checked while the beat runs beside it.
const LOCAL_POLL: Duration = Duration::from_millis(20);
/// How long queued work waits for the profile. A hub restart can welcome the agent before its
/// profile is ready, and the profile follows a few seconds later.
const PROFILE_WAIT: Duration = Duration::from_secs(60);

/// A local tool's result: its output, or why it failed.
type Outcome = Result<String, String>;

/// How the policy and the hub settle one of a model turn's tool calls.
enum Route {
    /// Refused for its input: the text the model sees, without asking the hub.
    Refused(String),
    /// Allowed by the policy with this id: it runs here.
    Local(String),
    /// Left to the hub, in the turn's batch.
    Hub,
}

/// How an allowed call runs: the program or the engine's launch spec its decision carries, and the policy the
/// harness decided it under (None for a call the hub decided).
#[derive(Default)]
struct Allowance {
    program: Option<Program>,
    engine: Option<EngineSpec>,
    policy: Option<String>,
}

/// The report a guest sends once it has run a call. A call the policy decided carries its record: its tool, its
/// input and the policy it was decided under.
fn tool_report(
    work: u64,
    call: &str,
    ok: bool,
    output: String,
    record: Option<&(String, String, Value)>,
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
    /// The hub's signed policy: the tools this agent runs without asking. None until the hub sends one, and then
    /// every call goes to the hub.
    policy: Option<PolicySnapshot>,
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
    /// Model calls the operator allowed beyond the profile's turn budget.
    extra_turns: u32,
    /// Set while a local tool runs, once the work item is cancelled or the agent is stopping: an
    /// engine session reads it and stops at once, instead of at the tool's end.
    tool_cancel: Arc<AtomicBool>,
    /// A stop that arrived while a local tool ran. The loop takes it at its next wait, as it
    /// would have taken the line had it arrived then.
    stopping: Option<String>,
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
    let writer = Lines::new(lines.get_ref().duplicate()?);
    let shared = Shared::new(writer, log);
    // The first stats report measures CPU from here, so the baseline is taken now.
    let baseline = sample_processes(Path::new(PROC_DIR));
    let (inbox_tx, inbox_rx) = mpsc::channel();
    let (operator_tx, operator_rx) = mpsc::channel();
    let shared_ref = &shared;
    let outcome = thread::scope(|scope| {
        scope.spawn(move || read_host(lines, shared_ref, inbox_tx, operator_tx));
        scope.spawn(move || {
            let exec = |name: &str, args: &Value| {
                tools::serve(workspace, name, args).map_err(|err| err.to_string())
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
            policy: None,
            queue: VecDeque::new(),
            threads: load_threads(workspace),
            clock: 0,
            current: None,
            cancelled: None,
            steers: Vec::new(),
            compress_requested: false,
            lane: None,
            compressed_size: 0,
            extra_turns: 0,
            tool_cancel: Arc::new(AtomicBool::new(false)),
            stopping: None,
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
        shared_ref.stop();
        outcome
    });
    outcome?;
    let calls = shared.calls();
    shared.say(&GuestMessage::Bye { calls })?;
    Ok(calls)
}

impl<S: Write + Send, W: Write + Send> Agent<'_, '_, S, W> {
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
                crate::shells::stop_all(self.workspace);
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

    /// Runs a local tool on its own thread, beating beside it and taking the
    /// host's messages meanwhile. A panicking tool is an error result, not a dead
    /// session.
    fn run_local(&mut self, job: impl FnOnce() -> Outcome + Send) -> Result<Outcome, SessionError> {
        self.tool_cancel.store(false, Ordering::SeqCst);
        thread::scope(|scope| -> Result<Outcome, SessionError> {
            let handle = scope.spawn(job);
            while !handle.is_finished() {
                if Instant::now() >= self.next_beat {
                    self.beat()?;
                }
                if Instant::now() >= self.next_stats {
                    self.report_stats()?;
                }
                self.hear_during_tool()?;
                thread::sleep(LOCAL_POLL);
            }
            Ok(handle
                .join()
                .unwrap_or_else(|_| Err("internal: the tool panicked".to_string())))
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
        self.extra_turns = 0;
        let result = self.drive(job.work, &profile, &mut messages);
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
            let auto = profile.compress_at > 0
                && size > profile.compress_at
                && size > self.compressed_size.saturating_add(profile.compress_at / 4);
            if self.compress_requested || auto {
                let reason = if self.compress_requested {
                    "asked"
                } else {
                    "auto"
                };
                self.compress_requested = false;
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
            let ChatResponse { content, stop, .. } = response;
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
                StopReason::EndTurn => return Ok((WorkStatus::Done, blocks_text(&content))),
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
            "Summarize this work so far for yourself, so you can continue it without the transcript: the goal, what you did, what you found, files you created or changed (paths), decisions, what failed, and the next steps. Be specific and complete; at most 600 words.\n\n{transcript}"
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
            system: (!profile.system.is_empty()).then(|| profile.system.clone()),
            messages: messages.to_vec(),
            tools: profile.tools.clone(),
            max_tokens: 0,
            temperature: None,
            effort: None,
        };
        self.say(&GuestMessage::ModelCall { req, work, request })?;
        let (ok, response, error) =
            self.await_message("model_result", |message| match message {
                HostMessage::ModelResult {
                    req: got,
                    ok,
                    response,
                    error,
                } if got == req => Ok((ok, response, error)),
                other => Err(other),
            })?;
        Ok(match (ok, response) {
            (true, Some(response)) => Ok(response),
            (true, None) => Err("the host sent no answer".to_string()),
            (false, _) => Err(error.unwrap_or_else(|| "the model call failed".to_string())),
        })
    }

    /// The turn's tool calls, in the model's order, each as its result text and whether it is an error. A call with
    /// invalid input is refused without asking the host. A call the policy allows runs here, reported under the policy
    /// it was decided by. The others go to the host in one batch, which answers once; then the calls run in order,
    /// one at a time, each as its decision says.
    fn tool_turn(
        &mut self,
        work: u64,
        uses: &[(String, String, Value, Option<Value>)],
    ) -> Result<Vec<(String, bool)>, Halt> {
        let policy = self.policy.clone();
        let routes: Vec<Route> = uses
            .iter()
            .map(|(_, name, input, meta)| {
                if let Some(reason) = invalid_input(input, meta.as_ref()) {
                    return Route::Refused(format!("INVALID_INPUT: {reason}"));
                }
                match &policy {
                    Some(policy) if policy.allow.iter().any(|tool| tool == name) => {
                        Route::Local(policy.id.clone())
                    }
                    _ => Route::Hub,
                }
            })
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
        let mut outcomes = Vec::with_capacity(uses.len());
        for ((id, name, input, _), route) in uses.iter().zip(routes) {
            let outcome = match route {
                Route::Refused(text) => (text, true),
                Route::Local(policy) => {
                    let allowance = Allowance {
                        policy: Some(policy),
                        ..Allowance::default()
                    };
                    self.run_allowed(work, id, name, input, allowance)?
                }
                Route::Hub => match decided.remove(id) {
                    Some(decision) => self.apply_decision(work, id, name, input, decision)?,
                    None => (
                        "the host sent no decision for this tool call".to_string(),
                        true,
                    ),
                },
            };
            outcomes.push(outcome);
        }
        Ok(outcomes)
    }

    /// The hub's decision on one call of the turn: an allowed call runs here and is reported; a done, pending or
    /// denied call is its result.
    fn apply_decision(
        &mut self,
        work: u64,
        id: &str,
        name: &str,
        input: &Value,
        decided: BatchDecision,
    ) -> Result<(String, bool), Halt> {
        let BatchDecision {
            decision,
            output,
            is_error,
            program,
            engine,
            ..
        } = decided;
        match decision {
            Decision::Allow => {
                let allowance = Allowance {
                    program,
                    engine,
                    policy: None,
                };
                self.run_allowed(work, id, name, input, allowance)
            }
            Decision::Done => Ok((cap_output(&output, MAX_TOOL_OUTPUT), is_error)),
            // Held for the owner (only an ACP engine's requests are held, and those wait on their own
            // nested ids). A model's call that comes back held is an error it can retry later.
            Decision::Pending => Ok((
                "the owner has not decided this tool call yet; try again later".to_string(),
                true,
            )),
            Decision::Deny if output.is_empty() => {
                Ok(("the host denied this tool call".to_string(), true))
            }
            Decision::Deny => Ok((output, true)),
        }
    }

    /// Runs one call the host allowed, or the policy did, and reports it. A call the policy decided carries its
    /// record in the report: its tool, its input and the policy it was decided under.
    fn run_allowed(
        &mut self,
        work: u64,
        id: &str,
        name: &str,
        input: &Value,
        allowance: Allowance,
    ) -> Result<(String, bool), Halt> {
        let Allowance {
            program,
            engine,
            policy,
        } = allowance;
        let workspace = self.workspace;
        // A library tool comes with its program; built-in tools run from this harness.
        let outcome = match program {
            Some(program) => self.run_local(move || library::run(workspace, &program, input))?,
            // An engine runs as an ACP session whose every action the hub checks. The hub's
            // launch spec, for an engine instance the owner configured, comes with the decision.
            None if name == "code_engine" => {
                let shared = self.shared;
                let cancel = Arc::clone(&self.tool_cancel);
                let call = id.to_string();
                // The engine's MCP server offers the profile's plaza tools, through the hub.
                let tools = self
                    .profile
                    .as_ref()
                    .map_or_else(Vec::new, |profile| profile.tools.clone());
                let protocol = self.profile.as_ref().map_or(1, |profile| {
                    crate::acp::run::protocol_offer(&profile.engine_transport)
                });
                self.run_local(move || {
                    let link = HubLink::new(shared, work, call, &cancel, tools);
                    crate::acp::run::run(workspace, input, &link, protocol, engine.as_ref())
                })?
            }
            None => self.run_local(move || run_guest_tool(workspace, name, input))?,
        };
        let (ok, output) = match outcome {
            Ok(output) => (true, output),
            Err(err) => (false, err),
        };
        let record = policy.map(|policy| (policy, name.to_string(), input.clone()));
        // A packaged tool travels to the host whole (the library files it from the report);
        // the model only needs to hear what was packaged.
        if name == "tool_package" && ok {
            self.say(&tool_report(work, id, ok, output.clone(), record.as_ref()))?;
            return Ok((library::summary(&output), false));
        }
        let output = cap_output(&output, MAX_TOOL_OUTPUT);
        self.say(&tool_report(work, id, ok, output.clone(), record.as_ref()))?;
        Ok((output, !ok))
    }
}

/// Why a tool call's input cannot run, found without asking the host: the
/// gateway could not parse the model's arguments, or they are not an object.
fn invalid_input(input: &Value, meta: Option<&Value>) -> Option<&'static str> {
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

/// The names [`run_guest_tool`] serves itself, besides the file tools of [`tools::serve`].
const GUEST_TOOLS: [&str; 26] = [
    "bash",
    "install_packages",
    "multi_edit",
    "glob",
    "grep",
    "bash_start",
    "bash_output",
    "bash_kill",
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
    "edit_file",
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

/// Runs a tool the host allowed, under the names the model knows. `edit_file`
/// takes `old` and `new` from the model, which the file tool calls `find` and
/// `replace`.
fn run_guest_tool(workspace: &Workspace, name: &str, input: &Value) -> Outcome {
    match name {
        "bash" => bash::run(workspace, input),
        "install_packages" => bash::install_packages(workspace, input),
        "multi_edit" => files::multi_edit(workspace, input),
        "glob" => search_tools::glob(workspace, input),
        "grep" => search_tools::grep(workspace, input),
        "bash_start" => shells::start(workspace, input),
        "bash_output" => shells::output(workspace, input),
        "bash_kill" => shells::kill(workspace, input),
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
        "edit_file" => {
            let args = edit_args(input)?;
            tools::serve(workspace, name, &args).map_err(|err| err.to_string())
        }
        // The owner's IDE tools come from the hub's operator, never from the model's own calls.
        name if name.starts_with("ide_") => Err(format!(
            "denied: {name} is an owner tool; the model is not offered it"
        )),
        _ => tools::serve(workspace, name, input).map_err(|err| err.to_string()),
    }
}

/// The file tool's arguments for the model's `{path, old, new}`.
fn edit_args(input: &Value) -> Result<Value, String> {
    let (Some(old), Some(new)) = (
        input.get("old").and_then(Value::as_str),
        input.get("new").and_then(Value::as_str),
    ) else {
        return Err("invalid input: edit_file needs string \"old\" and \"new\"".to_string());
    };
    let mut args = input.clone();
    args["find"] = json!(old);
    args["replace"] = json!(new);
    Ok(args)
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
fn paused_wait(n: u32) -> Duration {
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
                "name": "ada", "role": "builder", "model": "anthropic/x", "system": "be brief",
                "tools": [{"name": "bash", "description": "run", "input_schema": {"type": "object"}}],
                "max_turns": max_turns,
            },
        })
        .to_string()
    }

    fn work(id: u64, kind: &str, thread: &str, prompt: &str) -> String {
        json!({"type": "work", "work": id, "kind": kind, "thread": thread, "prompt": prompt})
            .to_string()
    }

    fn answer(req: u64, content: Value, stop: &str) -> String {
        json!({
            "type": "model_result", "req": req, "ok": true,
            "response": {"model": "anthropic/x", "content": content, "stop": stop,
                         "usage": {"input_tokens": 1, "output_tokens": 1}},
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
                    (first["req"].clone(), first["request"]["system"].clone()),
                    (json!(1), json!("be brief"))
                );
                assert_eq!(
                    first["request"]["messages"][0]["content"][0]["text"],
                    "say hi"
                );
                host.send(&answer(
                    1,
                    json!([tool_use("t1", "bash", json!({"command": "echo hi"}))]),
                    "tool_use",
                ))
                .unwrap();
                let batch = expect_type(host, "tool_batch");
                assert_eq!(
                    (
                        batch["calls"][0]["call"].clone(),
                        batch["calls"][0]["name"].clone()
                    ),
                    (json!("t1"), json!("bash"))
                );
                host.send(&decision("t1", "allow", "", false)).unwrap();
                let report = expect_type(host, "tool_report");
                assert_eq!(
                    (report["ok"].clone(), report["output"].clone()),
                    (json!(true), json!("hi\nexit 0"))
                );
                let second = expect_type(host, "model_call");
                assert_eq!(
                    second["request"]["messages"][2]["content"][0]["content"],
                    "hi\nexit 0"
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
                    json!([tool_use("d1", "bash", json!({"command": "rm -rf /"}))]),
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
                        "bash",
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
                    (json!(format!("f{n}")), json!(format!("{n}\nexit 0"))),
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
            host.send(&policy_line("p1", &["bash"])).unwrap();
            host.send(&work(1, "task", "t", "five things")).unwrap();
            expect_type(host, "model_call");
            let calls: Vec<Value> = (1..=5)
                .map(|n| {
                    tool_use(
                        &format!("l{n}"),
                        "bash",
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
                        json!(format!("{n}\nexit 0")),
                        json!("p1"),
                        json!("bash"),
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
            host.send(&policy_line("p2", &["bash"])).unwrap();
            host.send(&work(1, "task", "t", "mixed")).unwrap();
            expect_type(host, "model_call");
            host.send(&answer(
                1,
                json!([
                    tool_use("m1", "bash", json!({"command": "echo 1"})),
                    tool_use("m2", "search", json!({"pattern": "x"})),
                    tool_use("m3", "bash", json!({"command": "echo 3"})),
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
            for (call, output) in [("m1", "1\nexit 0"), ("m3", "3\nexit 0")] {
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
                vec![json!("1\nexit 0"), json!("3 hits"), json!("3\nexit 0")],
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
                json!([tool_use("h1", "bash", json!({"command": "echo 1"}))]),
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
            assert_eq!(call["request"]["system"], "be brief", "{call}");
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
                "name": "ada", "role": "builder", "model": "anthropic/x", "system": "be brief",
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
                json!({"action": "steer", "from": "mayor", "text": "focus on the parser"}),
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
                results[1]["text"], "[note from mayor]: focus on the parser",
                "{results}"
            );
            host.send(&answer(2, json!([text("found")]), "end_turn"))
                .unwrap();
            expect_type(host, "work_done");
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

    #[test]
    fn malformed_tool_input_is_refused_without_asking_the_host() {
        let workspace = scratch("invalid");
        run_hub(&workspace, |host| {
            host.send(&welcome(10)).unwrap();
            host.send(&work(1, "task", "t", "go")).unwrap();
            expect_type(host, "model_call");
            let not_an_object = tool_use("i1", "bash", json!("ls"));
            let gateway_meta = json!({"type": "tool_use", "id": "i2", "name": "bash", "input": {},
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
                        json!([tool_use(&call, "bash", json!({"command": "x"}))]),
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
                        json!("stuck: bash failed 3 times in a row with the same input")
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
                    json!([tool_use("c1", "bash", json!({"command": "echo hi"}))]),
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
                    json!([tool_use("c1", "bash", json!({"command": "sleep 1"}))]),
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
                json!([tool_use("slow", "bash", json!({"command": "sleep 1"}))]),
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
    fn the_model_edit_arguments_map_onto_the_file_tools_names() {
        let mapped = edit_args(&json!({"path": "a.txt", "old": "x", "new": "y"})).unwrap();
        assert_eq!(
            (mapped["find"].clone(), mapped["replace"].clone()),
            (json!("x"), json!("y"))
        );
        assert!(
            edit_args(&json!({"path": "a.txt", "old": "x"}))
                .unwrap_err()
                .starts_with("invalid input:")
        );
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
}
