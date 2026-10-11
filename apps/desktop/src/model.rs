//! The dashboard's headless core: the panes, the state, the reducer and the
//! poll arithmetic.
//!
//! Nothing here knows about iced, so the whole model — parsing outcomes, the
//! reducer, validation and the poll schedule — is unit-tested without a
//! display or a window. `main` turns an [`Effect`] into an iced [`Task`] and
//! nothing more.

use crate::api::{
    ApiError, Award, Board, Column, CreateRun, CreateTask, Endpoint, Entry, FinishOutcome,
    HubAgent, HubStatus, Journal, Machine, Posting, Run, RunState, RunsPage, StopOutcome, Task,
    TaskArtifact, TaskState, TasksPage, TellAnswer, Totals, TreasuryRead, VmState,
};
use crate::events::{AgoraKind, Event};
use crate::export::{
    self, CsvCall, CsvExport, Export, ExportCall, ExportTarget, run_report, run_transcript,
    vm_transcript,
};
use crate::form::{Field, Preset, SpawnForm};
use crate::history::{self, SessionHistory};
use crate::ledger::{TaskField, TaskForm};
use crate::map::Activity;
use crate::serial::{Feed, Merge};

/// How often a healthy daemon is polled unless `--refresh-ms` says otherwise.
pub const DEFAULT_REFRESH_MS: u64 = 2_000;

/// The fastest the register may be asked to poll.
pub const MIN_REFRESH_MS: u64 = 250;

/// The slowest the register may be asked to poll. It is also the ceiling the
/// backoff climbs to, so a slow poll is never sped up by a failure.
pub const MAX_REFRESH_MS: u64 = 30_000;

/// The longest the poll backs off to while calls keep failing.
pub const MAX_POLL_INTERVAL_MS: u64 = 30_000;

/// How long a good answer stays "live" before the register calls it stale.
pub const STALE_AFTER_MS: u64 = 5_000;

/// How many lines the console remembers for the up/down history arrows.
pub const HISTORY_DEPTH: usize = 64;

/// How many of a serial capture's last lines the run pane lays out. A capture
/// is unbounded in principle; the layout is not, so the tail is what is drawn
/// and the count left out is shown above it.
pub const SERIAL_TAIL_LINES: usize = 400;

/// How many lines of a task's artifact the ledger lays out. The daemon's read
/// cap bounds the content, but the layout is bounded too: the tail is drawn
/// and the lines left out are counted above it, exactly as the transcript
/// does.
pub const ARTIFACT_TAIL_LINES: usize = 400;

/// Which sheet the register is showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pane {
    /// The fleet register.
    Fleet,
    /// One run, with its VMs and serial.
    Run(String),
    /// The spawn form.
    Spawn,
    /// The node's machine, as `GET /api/v0/machine` reports it.
    Machine,
    /// The fleet as a map: the machine's substrate, run lanes and VM cells.
    Map,
    /// The role fleet: who is running right now, grouped by role.
    Roster,
    /// The plaza's ledger: every task, and the moves the daemon accepts on
    /// them — create, claim, finish, withdraw.
    Ledger,
    /// The plaza's treasury: the accounts, the totals and the double-entry
    /// journal. Read-only — every entry is the daemon's own.
    Treasury,
    /// The plaza's market: the calls its roled tasks offer, the bids on them
    /// and the award that closed each. Read-only — a bid and an award are made
    /// by the daemon's own surface.
    Market,
    /// The session's own figures, one observation per poll, as charts.
    Telemetry,
}

/// Which VM of which run the operator has selected: the console's target and
/// the map's pressed cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// The selected run's id.
    pub run: String,
    /// The selected VM's index within that run.
    pub vm: u32,
}

/// One direction of the console's history walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryStep {
    /// The older entry.
    Prev,
    /// The newer entry.
    Next,
}

/// A key the register acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    /// `r`: the fleet register.
    Register,
    /// `m`: the machine.
    Machine,
    /// `f`: the fleet map.
    Map,
    /// `w`: the roster — who is running right now.
    Roster,
    /// `l`: the ledger — the plaza's tasks.
    Ledger,
    /// `y`: the treasury — the accounts and the journal.
    Treasury,
    /// `u`: the market — the calls, their bids and their awards.
    Market,
    /// `t`: the session's telemetry.
    Telemetry,
    /// Escape: back to the register.
    Back,
    /// `?`: the help sheet, or away from it.
    Help,
    /// Up: the older console line.
    HistoryPrev,
    /// Down: the newer console line.
    HistoryNext,
}

/// The console's input line, its history and where in the history it stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Console {
    /// What is typed in the box.
    pub line: String,
    /// The lines already sent, oldest first.
    pub history: Vec<String>,
    /// Where the up/down arrows are, counting back from the end; `None` means
    /// the box holds what the operator typed rather than a history entry.
    pub walk: Option<usize>,
}

impl Console {
    /// Remembers a line: the newest entry, without repeating the previous one.
    pub fn remember(&mut self, line: &str) {
        if self.history.last().map(String::as_str) == Some(line) {
            return;
        }
        self.history.push(line.to_string());
        if self.history.len() > HISTORY_DEPTH {
            let excess = self.history.len() - HISTORY_DEPTH;
            self.history.drain(..excess);
        }
    }

    /// Walks the history and answers the line to put in the box.
    ///
    /// `Prev` steps to the older entry and stops at the oldest; `Next` steps
    /// back toward the newest and, past it, hands the box back empty. With no
    /// history at all there is nothing to walk, so the box keeps what it holds.
    pub fn walk(&mut self, step: HistoryStep) -> String {
        if self.history.is_empty() {
            self.walk = None;
            return self.line.clone();
        }
        let walked = match (step, self.walk) {
            (HistoryStep::Prev, None) => Some(0),
            (HistoryStep::Prev, Some(at)) => Some((at + 1).min(self.history.len() - 1)),
            (HistoryStep::Next, None) => None,
            (HistoryStep::Next, Some(0)) => None,
            (HistoryStep::Next, Some(at)) => Some(at - 1),
        };
        self.walk = walked;
        match walked {
            Some(at) => self.history[self.history.len() - 1 - at].clone(),
            None => String::new(),
        }
    }
}

/// One console write the runtime has to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleCall {
    /// The run whose VM is written to.
    pub run: String,
    /// The VM's index.
    pub vm: u32,
    /// The line to write, newline-terminated.
    pub data: String,
}

/// One claim the runtime has to make: an open task, and the live agent's guest
/// CID it is claimed as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimCall {
    /// The task to claim.
    pub id: String,
    /// The guest CID of the agent claiming it.
    pub agent_cid: u64,
}

/// One finish the runtime has to make: a claimed task, how it ended, and what
/// the agent said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishCall {
    /// The task to finish.
    pub id: String,
    /// How it ended.
    pub outcome: FinishOutcome,
    /// What the agent said, when it said anything.
    pub note: Option<String>,
}

/// One artifact read the runtime has to make: a task, and one path its agent
/// wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactCall {
    /// The task whose agent would read the path.
    pub id: String,
    /// The path to read, as the task's finish note named it.
    pub path: String,
}

/// The artifact the ledger's detail has read, or is reading, from the open
/// task's agent.
///
/// A read is a POST that drives the agent's `read_file`, so its answer is one
/// of four honest states, never a missing one: the read is in flight, the
/// agent read the file, the daemon refused it, or the daemon was never told
/// where the hub's admin surface is. The task it names is the task it was asked
/// of, so a read for one task is never drawn under another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactState {
    /// The row was pressed; the read is in flight.
    Loading {
        /// The task whose agent is reading.
        task: String,
        /// The path being read.
        path: String,
    },
    /// The agent read the file.
    Read {
        /// The task whose agent read it.
        task: String,
        /// What it read, and the terms it read it in.
        artifact: TaskArtifact,
    },
    /// The daemon refused the read, in its own words: the hub's role policy
    /// (`403`), the agent's own refusal (`422`), a task with no live agent to
    /// read it (`404`/`409`), or a daemon that could not be asked at all.
    Refused {
        /// The task whose artifact was asked for.
        task: String,
        /// The path that was asked for.
        path: String,
        /// Why, as the client reads it.
        error: ApiError,
    },
    /// The daemon was never told where the hub's admin surface is (`503`), so
    /// the read-through is switched off rather than refused. That is a
    /// different fact from a refusal, and is drawn as one.
    NotConfigured {
        /// The task whose artifact was asked for.
        task: String,
        /// The path that was asked for.
        path: String,
        /// The daemon's own words for it.
        message: String,
    },
}

impl ArtifactState {
    /// The task this read was asked of.
    pub fn task(&self) -> &str {
        match self {
            ArtifactState::Loading { task, .. } | ArtifactState::Read { task, .. } => task,
            ArtifactState::Refused { task, .. } | ArtifactState::NotConfigured { task, .. } => task,
        }
    }

    /// The path this read was asked for.
    pub fn path(&self) -> &str {
        match self {
            ArtifactState::Loading { path, .. } | ArtifactState::Refused { path, .. } => path,
            ArtifactState::NotConfigured { path, .. } => path,
            ArtifactState::Read { artifact, .. } => &artifact.path,
        }
    }

    /// Classifies one read's answer: a `503` is the read-through being off,
    /// everything else the daemon refuses is a refusal, and `Ok` is what the
    /// agent read.
    fn from_answer(task: &str, path: &str, result: Result<TaskArtifact, ApiError>) -> Self {
        match result {
            Ok(artifact) => ArtifactState::Read {
                task: task.to_string(),
                artifact,
            },
            // A daemon not told where the hub's admin surface is answers 503:
            // the read-through is off, not refused, and the console says which.
            Err(ApiError::Status {
                status: 503,
                message,
            }) => ArtifactState::NotConfigured {
                task: task.to_string(),
                path: path.to_string(),
                message,
            },
            Err(error) => ArtifactState::Refused {
                task: task.to_string(),
                path: path.to_string(),
                error,
            },
        }
    }
}

/// What the last action did, as a line in the register.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// Whether the action finished or failed.
    pub kind: NoticeKind,
    /// What to show.
    pub text: String,
}

/// How a notice reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    /// The action finished.
    Done,
    /// The action failed.
    Failed,
}

impl NoticeKind {
    /// The word the notice's stamp presses.
    pub fn stamp_label(self) -> &'static str {
        match self {
            NoticeKind::Done => "DONE",
            NoticeKind::Failed => "REFUSED",
        }
    }
}

/// How fresh the daemon's answers are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    /// Nothing has answered yet.
    Unknown,
    /// An answer arrived within [`STALE_AFTER_MS`].
    Live,
    /// The last answer is older than [`STALE_AFTER_MS`], but nothing failed.
    Stale,
    /// A call failed: the daemon is refusing or unreachable.
    Down,
}

impl Health {
    /// The word the connection stamp shows.
    pub fn label(self) -> &'static str {
        match self {
            Health::Unknown => "waiting",
            Health::Live => "live",
            Health::Stale => "stale",
            Health::Down => "down",
        }
    }

    /// The word the connection stamp presses, in caps.
    pub fn stamp_label(self) -> &'static str {
        match self {
            Health::Unknown => "WAITING",
            Health::Live => "LIVE",
            Health::Stale => "STALE",
            Health::Down => "DOWN",
        }
    }
}

/// The most a tool name may occupy, in bytes: the hub's own bound
/// (`crates/vsockd/src/admin.rs`, `MAX_TOOL_NAME`), mirrored so the console
/// refuses the same names the hub would.
pub const MAX_TOOL_NAME: usize = 256;

/// How long a `/tell` waits by default, in milliseconds: the hub's own default
/// (`DEFAULT_TELL_TIMEOUT_MS`), and the value the console's timeout box starts
/// at.
pub const DEFAULT_TELL_TIMEOUT_MS: u64 = 5_000;

/// The longest a drive may wait, in milliseconds: the hub's own ceiling
/// (`MAX_TELL_TIMEOUT_MS`), mirrored for the same reason.
pub const MAX_TELL_TIMEOUT_MS: u64 = 5_000;

/// One field of the roster console's drive form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConsoleField {
    /// The tool to run.
    Tool,
    /// The tool's arguments, as a JSON object.
    Args,
    /// How long to wait, in milliseconds.
    Timeout,
}

impl ConsoleField {
    /// The box's placeholder, naming what belongs in it.
    pub fn hint(self) -> &'static str {
        match self {
            ConsoleField::Tool => "the tool to run, e.g. uptime",
            ConsoleField::Args => r#"args as a JSON object, e.g. {"path":"."}"#,
            ConsoleField::Timeout => "ms to wait, 1–5000",
        }
    }
}

/// The operator's console on the roster: which connected agent it is aimed at,
/// the drive form's raw input, and the last drive answer and stop outcome.
///
/// It is opened by a row's 'drive' (or by confirming a stop) and aimed at that
/// row's guest CID — the key the hub's admin surface reaches an agent by. The
/// fields hold exactly what the operator typed; the JSON parse and the bounds
/// are checked on send, mirroring the hub's own contract.
#[derive(Debug, Clone, PartialEq)]
pub struct RosterConsole {
    /// The guest CID the console is aimed at.
    pub cid: u64,
    /// The tool-name box.
    pub tool: String,
    /// The args box: a JSON object, or empty for `{}`.
    pub args: String,
    /// The timeout box, in milliseconds.
    pub timeout: String,
    /// The last drive on this agent, from what the hub actually said.
    pub drive: Option<DriveState>,
    /// The last stop asked of this agent, if any.
    pub stop: Option<StopState>,
}

impl RosterConsole {
    /// A fresh console aimed at `cid`: an empty form, the hub's own default
    /// timeout, and nothing answered yet.
    pub fn new(cid: u64) -> Self {
        Self {
            cid,
            tool: String::new(),
            args: String::new(),
            timeout: DEFAULT_TELL_TIMEOUT_MS.to_string(),
            drive: None,
            stop: None,
        }
    }
}

/// What the operator's last drive on the roster console produced, from what
/// the hub's admin surface actually said.
#[derive(Debug, Clone, PartialEq)]
pub enum DriveState {
    /// The call is in flight.
    Driving {
        /// The agent it was asked of.
        cid: u64,
        /// The tool it asked for.
        name: String,
        /// The wait it will give the agent.
        timeout_ms: u64,
    },
    /// The agent answered: the tool's own `ok` and its output. `ok: false` is
    /// the tool declining — still an answer, its reason in `output`.
    Answered {
        /// The agent that answered.
        cid: u64,
        /// The tool that ran.
        name: String,
        /// Whether the tool succeeded.
        ok: bool,
        /// The tool's output — the agent's own words.
        output: String,
    },
    /// The hub refused the call itself, in its own words: the session's role
    /// not granting the tool (`403`, decided before anything reached the
    /// agent) or the request outside its contract (`400`/`404`/`409`/`504`).
    Refused {
        /// The agent the call named.
        cid: u64,
        /// The tool the call named.
        name: String,
        /// The hub's HTTP status.
        status: u16,
        /// The hub's own words for the refusal.
        message: String,
    },
    /// The admin surface itself could not be read: unreachable, or an answer
    /// this console cannot parse.
    Unreachable {
        /// The agent the call named.
        cid: u64,
        /// The tool the call named.
        name: String,
        /// Why it could not be read.
        message: String,
    },
    /// The form was refused before it was sent: this console's own validation,
    /// mirroring the hub's contract, so the operator learns it without a round
    /// trip — and nothing reached the agent.
    Rejected {
        /// The agent it was aimed at.
        cid: u64,
        /// What the form itself said.
        message: String,
    },
}

impl DriveState {
    /// The guest CID this state is about.
    pub fn cid(&self) -> u64 {
        match self {
            DriveState::Driving { cid, .. }
            | DriveState::Answered { cid, .. }
            | DriveState::Refused { cid, .. }
            | DriveState::Unreachable { cid, .. }
            | DriveState::Rejected { cid, .. } => *cid,
        }
    }

    /// The tool this state names, when it names one (a rejected form names
    /// none).
    pub fn name(&self) -> Option<&str> {
        match self {
            DriveState::Driving { name, .. }
            | DriveState::Answered { name, .. }
            | DriveState::Refused { name, .. }
            | DriveState::Unreachable { name, .. } => Some(name),
            DriveState::Rejected { .. } => None,
        }
    }

    /// The word the console presses as this state's mark.
    pub fn mark(&self) -> &'static str {
        match self {
            DriveState::Driving { .. } => "DRIVING",
            DriveState::Answered { ok: true, .. } => "ANSWERED",
            DriveState::Answered { ok: false, .. } => "TOOL DECLINED",
            DriveState::Refused { status: 403, .. } => "ROLE POLICY",
            DriveState::Refused { .. } => "REFUSED",
            DriveState::Unreachable { .. } => "UNREACHABLE",
            DriveState::Rejected { .. } => "NOT SENT",
        }
    }

    /// The meta line under the mark: the tool, the wait, and the CID — or the
    /// hub's status when the call was refused.
    pub fn meta(&self) -> String {
        match self {
            DriveState::Driving {
                cid,
                name,
                timeout_ms,
            } => format!("{name} · {timeout_ms} ms · cid {cid} · waiting for the agent"),
            DriveState::Answered {
                cid,
                name,
                ok: true,
                ..
            } => format!("{name} · cid {cid} · the agent's own answer"),
            DriveState::Answered {
                cid,
                name,
                ok: false,
                ..
            } => format!("{name} · cid {cid} · the tool declined"),
            DriveState::Refused {
                cid, name, status, ..
            } => format!("{name} · cid {cid} · HTTP {status}"),
            DriveState::Unreachable { cid, name, .. } => {
                format!("{name} · cid {cid} · the admin surface could not be read")
            }
            DriveState::Rejected { cid, .. } => format!("cid {cid} · nothing was sent"),
        }
    }

    /// The text the console's bounded region shows: the agent's own answer, or
    /// the hub's own words for a refusal — or `None` while the call is in
    /// flight or for a form that was never sent.
    ///
    /// Every byte here is somebody else's (the tool's output, the hub's
    /// message): the console displays it and never interprets it.
    pub fn text(&self) -> Option<&str> {
        match self {
            DriveState::Driving { .. } => None,
            DriveState::Answered { output, .. } => Some(output),
            DriveState::Refused { message, .. } => Some(message),
            DriveState::Unreachable { message, .. } => Some(message),
            DriveState::Rejected { message, .. } => Some(message),
        }
    }
}

/// What the operator's last stop on the roster console produced, from what the
/// hub's admin surface actually said.
#[derive(Debug, Clone, PartialEq)]
pub enum StopState {
    /// The stop is in flight.
    Stopping {
        /// The agent it was asked of.
        cid: u64,
    },
    /// The session ended: the guest's `bye` count. The agent's VM stays up.
    Stopped {
        /// The agent whose session ended.
        cid: u64,
        /// The calls the ended session had served.
        calls: u64,
    },
    /// The wait lapsed and the session is still up: **nothing was torn down**,
    /// and the hub's own reason says so.
    StillUp {
        /// The agent that did not end.
        cid: u64,
        /// The hub's own words for the lapse.
        reason: String,
    },
    /// The hub refused the stop, in its own words: a CID it does not serve
    /// (404) or has no live session for (409).
    Refused {
        /// The agent the stop named.
        cid: u64,
        /// The hub's HTTP status.
        status: u16,
        /// The hub's own words.
        message: String,
    },
    /// The admin surface itself could not be read.
    Unreachable {
        /// The agent the stop named.
        cid: u64,
        /// Why it could not be read.
        message: String,
    },
}

impl StopState {
    /// The guest CID this state is about.
    pub fn cid(&self) -> u64 {
        match self {
            StopState::Stopping { cid }
            | StopState::Stopped { cid, .. }
            | StopState::StillUp { cid, .. }
            | StopState::Refused { cid, .. }
            | StopState::Unreachable { cid, .. } => *cid,
        }
    }

    /// The word the console presses as this state's mark.
    pub fn mark(&self) -> &'static str {
        match self {
            StopState::Stopping { .. } => "STOPPING",
            StopState::Stopped { .. } => "STOPPED",
            StopState::StillUp { .. } => "NOT STOPPED",
            StopState::Refused { .. } => "REFUSED",
            StopState::Unreachable { .. } => "UNREACHABLE",
        }
    }

    /// The honest sentence the console shows: what happened, and — every time
    /// something did not end — that nothing was torn down.
    pub fn text(&self) -> String {
        match self {
            StopState::Stopping { cid } => {
                format!("asking the agent on cid {cid} to end its session…")
            }
            StopState::Stopped { calls, .. } => format!(
                "the session ended after {calls} call(s); the agent's VM stays up"
            ),
            StopState::StillUp { reason, .. } => {
                format!("not stopped: {reason} — nothing was torn down")
            }
            StopState::Refused { message, .. } => message.clone(),
            StopState::Unreachable { message, .. } => message.clone(),
        }
    }
}

/// One `POST /tell` to make on the hub's admin surface: the tool, its
/// arguments, and how long to wait — already validated against the hub's own
/// contract.
#[derive(Debug, Clone, PartialEq)]
pub struct TellCall {
    /// The agent to drive.
    pub cid: u64,
    /// The tool to run.
    pub name: String,
    /// The tool's arguments, a JSON object.
    pub args: serde_json::Value,
    /// How long to wait for the agent, in milliseconds.
    pub timeout_ms: u64,
}

/// What the reducer asks the runtime to do next.
///
/// Every field is an independent call, so the runtime can run them in any
/// order; the model never depends on which finished first.
///
/// No `Eq`: a drive carries the arguments as parsed JSON ([`serde_json::Value`]
/// is `PartialEq` but not `Eq`, since a number may be a float), which is the
/// only field here that cannot be totally ordered.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Effect {
    /// Poll `GET /api/v0/runs`.
    pub poll_list: bool,
    /// Fetch `GET /api/v0/machine`.
    pub poll_machine: bool,
    /// Fetch `GET /status` on the hub's status surface, when one is
    /// configured: the other half of the roster, read on its own schedule.
    pub poll_hub: bool,
    /// Fetch `GET /api/v0/runs/{id}`.
    pub fetch_run: Option<String>,
    /// Fetch each of these runs, with serial: every VM's rate for the map's
    /// pulse, the roster's rows and the telemetry pane's decomposition.
    pub fetch_map: Vec<String>,
    /// `DELETE /api/v0/runs/{id}`.
    pub kill: Option<String>,
    /// `POST /api/v0/runs` with this body.
    pub spawn: Option<CreateRun>,
    /// `POST /api/v0/runs/{id}/input` with this console write.
    pub input: Option<ConsoleCall>,
    /// Write this transcript with `std::fs` — the register's only local write.
    pub export: Option<ExportCall>,
    /// Write this session CSV with the same one write.
    pub csv: Option<CsvCall>,
    /// Poll `GET /api/v0/tasks`.
    pub poll_tasks: bool,
    /// Fetch `GET /api/v0/tasks/{id}`.
    pub fetch_task: Option<String>,
    /// Read the treasury: `GET /api/v0/treasury` and
    /// `GET /api/v0/treasury/journal` at once, on the treasury's own
    /// schedule.
    pub poll_treasury: bool,
    /// Read the market: `GET /api/v0/agora`, on the market's own schedule.
    pub poll_agora: bool,
    /// `POST /api/v0/tasks` with this body.
    pub create_task: Option<CreateTask>,
    /// `POST /api/v0/tasks/{id}/claim` as this agent.
    pub claim_task: Option<ClaimCall>,
    /// `POST /api/v0/tasks/{id}/finish` with this outcome and note.
    pub finish_task: Option<FinishCall>,
    /// `DELETE /api/v0/tasks/{id}`.
    pub withdraw_task: Option<String>,
    /// `POST /api/v0/tasks/{id}/artifact` with this path.
    pub artifact: Option<ArtifactCall>,
    /// `POST /tell` on the hub's admin surface with this drive: one tool on
    /// one connected agent.
    pub tell_agent: Option<TellCall>,
    /// `POST /stop` on the hub's admin surface for this guest CID: ask the
    /// agent to end its session.
    pub stop_agent: Option<u64>,
}

impl Effect {
    /// Nothing to do.
    pub fn none() -> Self {
        Self::default()
    }

    /// Poll the register.
    pub fn poll() -> Self {
        Self {
            poll_list: true,
            ..Self::default()
        }
    }

    /// Fetch the machine.
    pub fn machine() -> Self {
        Self {
            poll_machine: true,
            ..Self::default()
        }
    }

    /// Read the hub's status surface: who is connected right now.
    pub fn hub() -> Self {
        Self {
            poll_hub: true,
            ..Self::default()
        }
    }

    /// Fetch one run.
    pub fn fetch(id: impl Into<String>) -> Self {
        Self {
            fetch_run: Some(id.into()),
            ..Self::default()
        }
    }

    /// Fetch one run and poll the register.
    pub fn refresh(id: impl Into<String>) -> Self {
        Self {
            poll_list: true,
            fetch_run: Some(id.into()),
            ..Self::default()
        }
    }

    /// Open the map: the machine, and every run's serial for the pulse.
    pub fn map(ids: impl IntoIterator<Item = String>) -> Self {
        Self {
            poll_list: true,
            poll_machine: true,
            fetch_map: ids.into_iter().collect(),
            ..Self::default()
        }
    }

    /// Open the roster: the register, and every run's serial, so each role
    /// agent's own rate is a real figure and not a zero. The machine is not
    /// asked for — the roster draws no substrate.
    pub fn roster(ids: impl IntoIterator<Item = String>) -> Self {
        Self {
            poll_list: true,
            fetch_map: ids.into_iter().collect(),
            ..Self::default()
        }
    }

    /// Write one line to a VM's console.
    pub fn console(call: ConsoleCall) -> Self {
        Self {
            input: Some(call),
            ..Self::default()
        }
    }

    /// Read the ledger: every task, oldest first.
    pub fn ledger() -> Self {
        Self {
            poll_tasks: true,
            ..Self::default()
        }
    }

    /// Read the treasury: the accounts and their totals, and the journal
    /// beside them, in one read.
    pub fn treasury() -> Self {
        Self {
            poll_treasury: true,
            ..Self::default()
        }
    }

    /// Read the market: the board of calls, with each call's bids and its
    /// award.
    pub fn agora() -> Self {
        Self {
            poll_agora: true,
            ..Self::default()
        }
    }

    /// Read one task.
    pub fn task(id: impl Into<String>) -> Self {
        Self {
            fetch_task: Some(id.into()),
            ..Self::default()
        }
    }

    /// Read one artifact through the task's agent.
    pub fn artifact(id: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            artifact: Some(ArtifactCall {
                id: id.into(),
                path: path.into(),
            }),
            ..Self::default()
        }
    }

    /// Drive one tool on the hub's admin surface: one `POST /tell`.
    pub fn tell(call: TellCall) -> Self {
        Self {
            tell_agent: Some(call),
            ..Self::default()
        }
    }

    /// Ask one agent to end its session on the hub's admin surface: one
    /// `POST /stop`.
    pub fn stop(cid: u64) -> Self {
        Self {
            stop_agent: Some(cid),
            ..Self::default()
        }
    }

    /// Whether there is nothing to do.
    pub fn is_idle(&self) -> bool {
        *self == Self::default()
    }
}

/// Everything the dashboard reacts to.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// The poll timer fired, at `at_ms` milliseconds since the Unix epoch.
    Tick {
        /// The wall-clock time of the tick.
        at_ms: u64,
    },
    /// `GET /api/v0/runs` answered.
    Runs {
        /// When the answer arrived.
        at_ms: u64,
        /// The answer.
        result: Result<RunsPage, ApiError>,
    },
    /// `GET /api/v0/runs/{id}` answered.
    Run {
        /// When the answer arrived.
        at_ms: u64,
        /// Which run was asked about.
        id: String,
        /// The answer.
        result: Result<Run, ApiError>,
    },
    /// `GET /api/v0/machine` answered.
    Machine {
        /// When the answer arrived.
        at_ms: u64,
        /// The answer.
        result: Result<Machine, ApiError>,
    },
    /// `GET /status` on the hub's status surface answered: who is connected
    /// right now, or why the hub could not be read.
    Hub {
        /// When the answer arrived.
        at_ms: u64,
        /// The answer.
        result: Result<HubStatus, ApiError>,
    },
    /// A roster row's 'drive' was pressed: open the operator console aimed at
    /// that agent's guest CID.
    SelectAgent(u64),
    /// The roster console's drive form was edited.
    DriveEdit {
        /// Which box.
        field: ConsoleField,
        /// Its new text.
        value: String,
    },
    /// The roster console's send button was pressed: validate the form and
    /// drive the tool.
    DriveSend,
    /// The roster console was closed.
    DriveClose,
    /// A roster row's 'stop' was pressed; confirming it asks the agent to end
    /// its session.
    StopClicked(u64),
    /// `POST /tell` on the hub's admin surface answered.
    Told {
        /// When the answer arrived.
        at_ms: u64,
        /// The agent that was driven.
        cid: u64,
        /// The tool that was asked for.
        name: String,
        /// The answer, or why it did not arrive.
        result: Result<TellAnswer, ApiError>,
    },
    /// `POST /stop` on the hub's admin surface answered.
    Stopped {
        /// When the answer arrived.
        at_ms: u64,
        /// The agent the stop named.
        cid: u64,
        /// The outcome, or why it did not arrive.
        result: Result<StopOutcome, ApiError>,
    },
    /// `POST /api/v0/runs/{id}/input` answered.
    Input {
        /// When the answer arrived.
        at_ms: u64,
        /// Which run was written to.
        id: String,
        /// Which of its VMs.
        vm: u32,
        /// The answer: the run, with its VMs' serial.
        result: Result<Run, ApiError>,
    },
    /// A key was pressed.
    Key(KeyAction),
    /// A map cell was pressed: open its run with its VM selected.
    Select {
        /// The run whose cell was pressed.
        run: String,
        /// The VM whose cell was pressed.
        vm: u32,
    },
    /// A VM's row was pressed: target the console at it.
    SelectVm(u32),
    /// The console's input line was edited.
    ConsoleEdit(String),
    /// The console's send button was pressed.
    ConsoleSend,
    /// One of the guest's quick commands was pressed.
    ConsoleQuick(&'static str),
    /// The console's history was walked.
    ConsoleHistory(HistoryStep),
    /// Show a pane.
    Show(Pane),
    /// Whether the run pane keeps following the serial.
    Follow(bool),
    /// The kill button was pressed; confirming it kills the run.
    KillClicked(String),
    /// `DELETE /api/v0/runs/{id}` answered.
    Killed {
        /// When the answer arrived.
        at_ms: u64,
        /// Which run was killed.
        id: String,
        /// The answer.
        result: Result<Run, ApiError>,
    },
    /// The spawn form was submitted; its validity decides whether it is sent.
    SpawnClicked,
    /// `POST /api/v0/runs` answered.
    Spawned {
        /// When the answer arrived.
        at_ms: u64,
        /// The answer.
        result: Result<Run, ApiError>,
    },
    /// A spawn form field was edited.
    Edit {
        /// Which field.
        field: Field,
        /// Its new text.
        value: String,
    },
    /// A quick-boot preset was pressed: it fills the form.
    Preset(Preset),
    /// The export path box was edited.
    ExportEdited(String),
    /// The export target was switched.
    ExportTarget(ExportTarget),
    /// The export button was pressed.
    Export,
    /// A transcript write answered.
    Exported {
        /// When the answer arrived.
        at_ms: u64,
        /// What the write covered: one VM, the run's transcripts, or its
        /// report.
        target: ExportTarget,
        /// Which run the transcript was of.
        run: String,
        /// Which VM, or `None` for the whole run.
        vm: Option<u32>,
        /// Where it was written.
        path: String,
        /// How many bytes were written, or why they were not.
        result: Result<usize, String>,
    },
    /// The help sheet was toggled by its button.
    Help(bool),
    /// A run was picked for the telemetry pane, or `None` for the whole fleet.
    TelemetryPick(Option<String>),
    /// The session CSV's path box was edited.
    CsvEdited(String),
    /// The CSV export button was pressed.
    CsvExport,
    /// A session CSV write answered.
    CsvExported {
        /// When the answer arrived.
        at_ms: u64,
        /// Where it was written.
        path: String,
        /// How many bytes were written, or why they were not.
        result: Result<usize, String>,
    },
    /// Dismiss the notice line.
    DismissNotice,
    /// `GET /api/v0/tasks` answered.
    Tasks {
        /// When the answer arrived.
        at_ms: u64,
        /// The answer.
        result: Result<TasksPage, ApiError>,
    },
    /// The treasury answered: the accounts with their totals and the journal,
    /// read together.
    Treasury {
        /// When the answer arrived.
        at_ms: u64,
        /// The answer, or why the read did not happen.
        result: Result<TreasuryRead, ApiError>,
    },
    /// The market answered: the board of calls, with each call's bids and its
    /// award.
    Agora {
        /// When the answer arrived.
        at_ms: u64,
        /// The answer, or why the read did not happen.
        result: Result<Board, ApiError>,
    },
    /// `GET /api/v0/tasks/{id}` answered: one task, read again.
    Task {
        /// When the answer arrived.
        at_ms: u64,
        /// Which task was asked about.
        id: String,
        /// The answer.
        result: Result<Task, ApiError>,
    },
    /// A ledger row was pressed: open that task's detail.
    SelectTask(String),
    /// The detail's refresh button was pressed.
    TaskRefresh,
    /// A create-form field was edited.
    TaskEdit {
        /// Which field.
        field: TaskField,
        /// Its new text.
        value: String,
    },
    /// The ledger's create button was pressed.
    TaskCreate,
    /// `POST /api/v0/tasks` answered.
    TaskCreated {
        /// When the answer arrived.
        at_ms: u64,
        /// The answer.
        result: Result<Task, ApiError>,
    },
    /// The claim field was edited: the agent CID the task would be claimed as.
    ClaimCidEdit(String),
    /// The claim button was pressed.
    TaskClaim,
    /// `POST /api/v0/tasks/{id}/claim` answered.
    TaskClaimed {
        /// When the answer arrived.
        at_ms: u64,
        /// Which task was claimed.
        id: String,
        /// The guest CID it was claimed as.
        cid: u64,
        /// The answer.
        result: Result<Task, ApiError>,
    },
    /// The finish note was edited.
    FinishNoteEdit(String),
    /// A finish button was pressed, for one of the two outcomes.
    TaskFinish(FinishOutcome),
    /// `POST /api/v0/tasks/{id}/finish` answered.
    TaskFinished {
        /// When the answer arrived.
        at_ms: u64,
        /// Which task was finished.
        id: String,
        /// How it ended.
        outcome: FinishOutcome,
        /// The answer.
        result: Result<Task, ApiError>,
    },
    /// The withdraw button was pressed; confirming it withdraws the task.
    TaskWithdrawClicked(String),
    /// `DELETE /api/v0/tasks/{id}` answered.
    TaskWithdrawn {
        /// When the answer arrived.
        at_ms: u64,
        /// Which task was withdrawn.
        id: String,
        /// The answer.
        result: Result<Task, ApiError>,
    },
    /// An artifact row was pressed: read that path through the open task's
    /// agent.
    TaskArtifactRead(String),
    /// `POST /api/v0/tasks/{id}/artifact` answered.
    TaskArtifact {
        /// When the answer arrived.
        at_ms: u64,
        /// Which task's agent was asked.
        id: String,
        /// Which path was asked for.
        path: String,
        /// The answer: what the agent read, or why it did not.
        result: Result<TaskArtifact, ApiError>,
    },
    /// The daemon's event stream delivered an event, at `at_ms` milliseconds
    /// since the Unix epoch (when the console read it, since the frames carry
    /// no clock of their own).
    ///
    /// This is the whole of the stream's surface: the events are the deltas
    /// between polls, so they move the state the polls already move rather
    /// than introducing a second kind of truth.
    Event {
        /// When the frame was read off the socket.
        at_ms: u64,
        /// What it carried.
        event: Event,
    },
}

impl Message {
    /// The wall-clock time this message carries, when it carries one.
    ///
    /// An event carries one: a frame read off the socket is the daemon
    /// answering, so the register's clock moves with it and the stamp stays
    /// fresh between polls. What it does *not* do is touch the poll's own
    /// schedule or its failures — see [`Model::heard`].
    pub fn at_ms(&self) -> Option<u64> {
        match self {
            Message::Tick { at_ms }
            | Message::Runs { at_ms, .. }
            | Message::Run { at_ms, .. }
            | Message::Machine { at_ms, .. }
            | Message::Hub { at_ms, .. }
            | Message::Told { at_ms, .. }
            | Message::Stopped { at_ms, .. }
            | Message::Input { at_ms, .. }
            | Message::Killed { at_ms, .. }
            | Message::Spawned { at_ms, .. }
            | Message::Exported { at_ms, .. }
            | Message::CsvExported { at_ms, .. }
            | Message::Tasks { at_ms, .. }
            | Message::Treasury { at_ms, .. }
            | Message::Agora { at_ms, .. }
            | Message::Task { at_ms, .. }
            | Message::TaskCreated { at_ms, .. }
            | Message::TaskClaimed { at_ms, .. }
            | Message::TaskFinished { at_ms, .. }
            | Message::TaskWithdrawn { at_ms, .. }
            | Message::TaskArtifact { at_ms, .. }
            | Message::Event { at_ms, .. } => Some(*at_ms),
            Message::Show(_)
            | Message::Key(_)
            | Message::Select { .. }
            | Message::SelectVm(_)
            | Message::SelectAgent(_)
            | Message::DriveEdit { .. }
            | Message::DriveSend
            | Message::DriveClose
            | Message::StopClicked(_)
            | Message::ConsoleEdit(_)
            | Message::ConsoleSend
            | Message::ConsoleQuick(_)
            | Message::ConsoleHistory(_)
            | Message::Follow(_)
            | Message::KillClicked(_)
            | Message::SpawnClicked
            | Message::Edit { .. }
            | Message::Preset(_)
            | Message::ExportEdited(_)
            | Message::ExportTarget(_)
            | Message::Export
            | Message::Help(_)
            | Message::TelemetryPick(_)
            | Message::CsvEdited(_)
            | Message::CsvExport
            | Message::SelectTask(_)
            | Message::TaskRefresh
            | Message::TaskEdit { .. }
            | Message::TaskCreate
            | Message::ClaimCidEdit(_)
            | Message::TaskClaim
            | Message::FinishNoteEdit(_)
            | Message::TaskFinish(_)
            | Message::TaskWithdrawClicked(_)
            | Message::TaskArtifactRead(_)
            | Message::DismissNotice => None,
        }
    }
}

/// The dashboard's state.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    /// The daemon this register is talking to.
    pub endpoint: Endpoint,
    /// The hub's status surface, when `--hub-url` named one: a second
    /// read-only process, on a second loopback port, that says who is
    /// connected — as opposed to the register's word for who should be. `None`
    /// is a console with no hub half.
    pub hub: Option<Endpoint>,
    /// The hub's admin surface, when `--hub-admin-url` named one: the second
    /// process the operator's controls drive (`POST /tell`, `POST /stop`).
    /// `None` — the default — is a roster with no operator controls at all.
    pub hub_admin: Option<Endpoint>,
    /// The hub's last answer, or `None` when it has not been read or its last
    /// read failed.
    ///
    /// A connection list is a statement about *now*, so a failed read clears
    /// it rather than drawing yesterday's sessions as if they were here: the
    /// pane then says the hub is unreachable and claims nothing about who is
    /// connected.
    pub hub_status: Option<HubStatus>,
    /// Why the last hub call failed, if it did.
    pub hub_error: Option<String>,
    /// When the hub last answered, in milliseconds since the Unix epoch.
    pub hub_last_ok_ms: Option<u64>,
    /// When the hub's status surface should be read next.
    pub hub_next_at_ms: u64,
    /// How many hub calls have failed in a row: the hub's own backoff, since
    /// the hub is a second process and its failures are not the daemon's.
    pub hub_failures: u32,
    /// Which sheet is showing.
    pub pane: Pane,
    /// The register: every run, as the list endpoint reports it.
    pub runs: Vec<Run>,
    /// The selected run, as the single-run endpoint reports it (with serial).
    pub detail: Option<Run>,
    /// The node's machine, as the machine endpoint reports it.
    pub machine: Option<Machine>,
    /// Whether the run pane follows the serial as it grows.
    pub follow: bool,
    /// Which run and VM the console targets, and which map cell is pressed.
    pub selection: Option<Selection>,
    /// The console's input line, its history and its position in that history.
    pub console: Console,
    /// The transcript export: what it covers, and the path in its box.
    pub export: Export,
    /// The session CSV export: the path in its box.
    pub csv: CsvExport,
    /// The session's own observations: one per poll that saw something.
    pub history: SessionHistory,
    /// The run the telemetry pane is decomposed by, when one is picked.
    pub telemetry_run: Option<String>,
    /// Whether the help sheet is up, over whatever pane is showing.
    pub help: bool,
    /// Every VM's serial rate, from the deltas between polls.
    pub activity: Activity,
    /// Every VM's capture, as the stream's serial deltas move it: what makes
    /// an event's bytes land in the transcript exactly once.
    pub serial: Feed,
    /// Whether an event has already asked the register to be polled and the
    /// answer has not landed: a burst of events naming a run the register has
    /// not seen yet is one poll, not one per event.
    pub stream_asked: bool,
    /// The plaza's ledger: every task, oldest first, as the daemon reports it.
    pub tasks: Vec<Task>,
    /// Whether the ledger has ever answered. The counts the index prints are
    /// unknown until it has, which is not the same fact as an empty ledger.
    pub ledger_read: bool,
    /// The task the ledger has opened: the row its detail is drawn from.
    pub task_selected: Option<String>,
    /// The ledger's create form.
    pub task_form: TaskForm,
    /// The create form's errors, from the last submission.
    pub task_errors: Vec<(TaskField, String)>,
    /// The agent CID typed into the claim box: the agent the open task would
    /// be claimed as.
    pub claim_cid: String,
    /// The note typed into the finish box.
    pub finish_note: String,
    /// The task whose withdraw button was pressed once and is waiting for a
    /// second press.
    pub armed_task: Option<String>,
    /// The artifact the ledger's detail has read, or is reading, for the open
    /// task — a file the task's agent wrote, read back out of its workspace.
    /// `None` before any of the note's paths has been pressed.
    pub artifact: Option<ArtifactState>,
    /// Whether a task event has already asked for the ledger and the answer
    /// has not landed: a burst of events naming a task the ledger does not
    /// hold is one read, not one per event.
    pub ledger_asked: bool,
    /// The plaza's treasury: the accounts, the totals and the journal, as the
    /// daemon's own read reported them. `None` until the pane's first read
    /// answers — a treasury nobody has read is not an empty treasury.
    ///
    /// The last read is kept when a later read fails: the journal is
    /// append-only, so a read that is a few seconds old is incomplete rather
    /// than wrong, and the pane says which it is.
    pub treasury: Option<TreasuryRead>,
    /// Why the treasury's last read failed, if it did.
    pub treasury_error: Option<String>,
    /// When the treasury last answered, in milliseconds since the Unix epoch.
    pub treasury_last_ok_ms: Option<u64>,
    /// When the treasury should be read next.
    pub treasury_next_at_ms: u64,
    /// How many treasury reads have failed in a row: the treasury's own
    /// backoff, so a daemon that serves the register but not the treasury
    /// never marks the register down.
    pub treasury_failures: u32,
    /// Whether a treasury frame has already asked for the read and the answer
    /// has not landed: a burst of entries is one read, not one per entry.
    pub treasury_asked: bool,
    /// The plaza's market: the calls with their bids and their awards, as the
    /// daemon's board reported them. `None` until the pane's first read
    /// answers — a board nobody has read is not an empty board.
    ///
    /// The last read is kept when a later read fails: a bid the daemon already
    /// recorded is a fact, so an older board is incomplete rather than wrong,
    /// and the pane says which it is.
    pub agora: Option<Board>,
    /// Why the market's last read failed, if it did.
    pub agora_error: Option<String>,
    /// When the market last answered, in milliseconds since the Unix epoch.
    pub agora_last_ok_ms: Option<u64>,
    /// When the market should be read next.
    pub agora_next_at_ms: u64,
    /// How many market reads have failed in a row: the market's own backoff,
    /// so a daemon that serves the register but not the agora never marks the
    /// register down.
    pub agora_failures: u32,
    /// Whether an agora frame has already asked for the read and the answer
    /// has not landed: a burst of bids is one read, not one per bid.
    pub agora_asked: bool,
    /// How often a healthy daemon is polled, in milliseconds.
    pub refresh_ms: u64,
    /// The spawn form's raw input.
    pub form: SpawnForm,
    /// The spawn form's errors, from the last submission.
    pub errors: Vec<(Field, String)>,
    /// The run whose kill button was pressed once and is waiting for a second
    /// press.
    pub armed: Option<String>,
    /// The roster's operator console: which connected agent it is aimed at,
    /// its drive form, and the last drive answer and stop outcome. `None`
    /// until a row's 'drive' is pressed — and always `None` with no
    /// `--hub-admin-url`, since the controls are absent then.
    pub agent_console: Option<RosterConsole>,
    /// The agent whose 'stop' button was pressed once and is waiting for a
    /// second press.
    pub armed_stop: Option<u64>,
    /// The last action's outcome.
    pub notice: Option<Notice>,
    /// The wall-clock time of the last message, in milliseconds since the Unix
    /// epoch.
    pub now_ms: u64,
    /// When the register should be polled next.
    pub next_poll_at_ms: u64,
    /// How many register polls have gone out: the register's folio cursor.
    pub polls: u64,
    /// When the last call succeeded.
    pub last_ok_ms: Option<u64>,
    /// Why the last call failed.
    pub last_error: Option<String>,
    /// How many calls have failed in a row.
    pub failures: u32,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            endpoint: Endpoint::loopback(),
            hub: None,
            hub_admin: None,
            hub_status: None,
            hub_error: None,
            hub_last_ok_ms: None,
            hub_next_at_ms: 0,
            hub_failures: 0,
            pane: Pane::Fleet,
            runs: Vec::new(),
            detail: None,
            machine: None,
            follow: true,
            selection: None,
            console: Console::default(),
            export: Export::default(),
            csv: CsvExport::default(),
            history: SessionHistory::default(),
            telemetry_run: None,
            help: false,
            activity: Activity::default(),
            serial: Feed::new(),
            stream_asked: false,
            tasks: Vec::new(),
            ledger_read: false,
            task_selected: None,
            task_form: TaskForm::default(),
            task_errors: Vec::new(),
            claim_cid: String::new(),
            finish_note: String::new(),
            armed_task: None,
            artifact: None,
            ledger_asked: false,
            treasury: None,
            treasury_error: None,
            treasury_last_ok_ms: None,
            treasury_next_at_ms: 0,
            treasury_failures: 0,
            treasury_asked: false,
            agora: None,
            agora_error: None,
            agora_last_ok_ms: None,
            agora_next_at_ms: 0,
            agora_failures: 0,
            agora_asked: false,
            refresh_ms: DEFAULT_REFRESH_MS,
            form: SpawnForm::default(),
            errors: Vec::new(),
            armed: None,
            agent_console: None,
            armed_stop: None,
            notice: None,
            now_ms: 0,
            next_poll_at_ms: 0,
            polls: 0,
            last_ok_ms: None,
            last_error: None,
            failures: 0,
        }
    }
}

impl Model {
    /// A model showing the fleet register, polling as soon as the timer first
    /// fires.
    pub fn new() -> Self {
        Self::default()
    }

    /// The same, talking to `endpoint`.
    pub fn at(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            ..Self::default()
        }
    }

    /// The same, polling every `refresh_ms` milliseconds.
    pub fn at_cadence(endpoint: Endpoint, refresh_ms: u64) -> Self {
        Self {
            endpoint,
            refresh_ms,
            ..Self::default()
        }
    }

    /// The same, with the hub's status surface to read as well: the roster's
    /// second half. `None` leaves the console exactly as it was before there
    /// was a hub.
    pub fn with_hub(mut self, hub: Option<Endpoint>) -> Self {
        self.hub = hub;
        self
    }

    /// Whether a hub's status surface is configured: whether the roster has a
    /// connected half at all.
    pub fn has_hub(&self) -> bool {
        self.hub.is_some()
    }

    /// The same, with the hub's admin surface to drive as well: the roster's
    /// operator controls. `None` leaves the roster read-only, exactly as it
    /// was before the controls existed.
    pub fn with_hub_admin(mut self, hub_admin: Option<Endpoint>) -> Self {
        self.hub_admin = hub_admin;
        self
    }

    /// Whether the hub's admin surface is configured: whether the roster
    /// carries the operator's controls at all. With no admin url the controls
    /// are absent, not drawn-but-disabled.
    pub fn has_hub_admin(&self) -> bool {
        self.hub_admin.is_some()
    }

    /// The register's first effect: the first poll, and the machine once, so
    /// the index's memory reading has the host's total to measure against
    /// from the start rather than after a visit to the machine sheet.
    pub fn boot(&mut self, at_ms: u64) -> Effect {
        let mut effect = self.apply(Message::Tick { at_ms });
        effect.poll_machine = true;
        effect
    }

    /// The moment a run's age is read against: the wall clock of its end when
    /// it ended, and the register's own clock while it is still live — a fleet
    /// run that has been `running` for days is exactly as old as the register's
    /// clock says, never as old as a missing end would make it.
    pub fn run_reference_ms(&self, run: &Run) -> u64 {
        run.ended_at.unwrap_or(self.now_ms)
    }

    /// How long `run` has been (or was) alive, in milliseconds.
    pub fn run_elapsed_ms(&self, run: &Run) -> u64 {
        self.run_reference_ms(run).saturating_sub(run.created_at)
    }

    /// How long the daemon has been up, as the machine reports it.
    pub fn machine_uptime_ms(&self) -> Option<u64> {
        self.machine
            .as_ref()
            .map(|machine| self.now_ms.saturating_sub(machine.started_at_ms))
    }

    /// The VM the console writes to in `run_id`: the selection when it names
    /// that run, and the run's first VM otherwise.
    pub fn target_vm(&self, run_id: &str) -> u32 {
        match &self.selection {
            Some(selection) if selection.run == run_id => selection.vm,
            _ => 0,
        }
    }

    /// Which VM of the run the console holds: the target's place in `run`.
    pub fn target_vm_in<'a>(&self, run: &'a Run) -> Option<&'a crate::api::Vm> {
        let vm = self.target_vm(&run.id);
        run.vms.iter().find(|candidate| candidate.id == vm)
    }

    /// The capture the register is holding for the VM the console targets:
    /// where the stream's serial deltas land.
    pub fn target_capture<'a>(&self, run: &'a Run) -> &'a str {
        self.target_vm_in(run)
            .and_then(|vm| vm.serial.as_deref())
            .unwrap_or("")
    }

    /// How many VMs of the register are up.
    pub fn running_vms(&self) -> usize {
        self.runs.iter().map(Run::live_vms).sum()
    }

    /// How many of the register's runs have vsock devices, and so a guest CID.
    pub fn vsock_runs(&self) -> usize {
        self.runs.iter().filter(|run| run.base_cid().is_some()).count()
    }

    /// Every guest CID in the register, once each, in ascending order.
    pub fn guest_cids(&self) -> Vec<u64> {
        let mut cids: Vec<u64> = self
            .runs
            .iter()
            .flat_map(|run| run.vms.iter().filter_map(|vm| vm.guest_cid))
            .collect();
        cids.sort_unstable();
        cids.dedup();
        cids
    }

    /// The guest memory the register's live VMs have committed, in MiB: what
    /// the host has been asked to hold right now.
    pub fn committed_mib(&self) -> u64 {
        history::committed_mib(&self.runs)
    }

    /// The fleet's serial throughput right now: every running VM's smoothed
    /// rate, added up.
    pub fn fleet_lps(&self) -> f32 {
        history::fleet_lps(&self.runs, &self.activity)
    }

    /// The run the telemetry pane is decomposed by, when it is still in the
    /// register: a pick that has left it falls back to the whole fleet.
    pub fn telemetry_pick(&self) -> Option<&str> {
        self.telemetry_run
            .as_deref()
            .filter(|id| self.runs.iter().any(|run| run.id == *id))
    }

    /// The task the ledger has opened, when the ledger still holds it: the row
    /// its detail is drawn from, and the row its actions act on.
    pub fn selected_task(&self) -> Option<&Task> {
        let id = self.task_selected.as_deref()?;
        self.tasks.iter().find(|task| task.id == id)
    }

    /// The agents a claim would be accepted from right now: every VM the
    /// register shows running that carries a role, in run-then-VM order, with
    /// the guest CID it is reached by.
    ///
    /// The daemon accepts a claim only from an agent its register shows
    /// running, so these are exactly the CIDs a claim can be made as. The
    /// ledger offers them rather than leaving the operator to open the roster
    /// and type a number the daemon will refuse.
    pub fn live_agents(&self) -> Vec<(u64, &str)> {
        self.runs
            .iter()
            .flat_map(|run| run.vms.iter())
            .filter(|vm| vm.state == VmState::Running)
            .filter_map(|vm| vm.guest_cid.zip(vm.role.as_deref()))
            .collect()
    }

    /// The role fleet as the roster pane lists it: every role agent in the
    /// register, joined to the hub's connected sessions when a hub is
    /// configured and has answered, grouped by role.
    ///
    /// Empty when neither surface reports a role agent — an older daemon that
    /// stamps no role, and a hub nobody is connected to.
    pub fn roster(&self) -> Vec<RosterGroup> {
        roster(&self.runs, &self.activity, self.hub_status.as_ref())
    }

    /// How many role agents are up right now: what "who is running right now"
    /// counts.
    pub fn roster_live(&self) -> usize {
        self.runs
            .iter()
            .flat_map(|run| run.vms.iter())
            .filter(|vm| vm.role.is_some() && vm.state == VmState::Running)
            .count()
    }

    /// The role the roster files the agent with this guest CID under, when the
    /// roster reports it: what the operator console names the agent by. `None`
    /// for a CID no roster row carries — which is what a console aimed at an
    /// agent the roster has since dropped reads.
    pub fn roster_role(&self, cid: u64) -> Option<String> {
        self.roster()
            .into_iter()
            .flat_map(|group| group.agents)
            .find(|agent| agent.cid() == Some(cid))
            .map(|agent| agent.role().to_string())
    }

    /// How many agents the hub reports connected right now, when it has
    /// answered.
    pub fn hub_connected(&self) -> Option<usize> {
        self.hub_status.as_ref().map(|status| status.agents.len())
    }

    /// Whether the live VMs have been asked for more guest memory than the
    /// host has, as the machine reports it. Unknown machine: not over.
    pub fn overcommitted(&self) -> bool {
        self.machine
            .as_ref()
            .is_some_and(|machine| self.committed_mib() > machine.memory_mib)
    }

    /// The register's memory line: what the fleet has committed, against the
    /// host's total once the machine has been reported.
    pub fn committed_line(&self) -> String {
        let committed = memory_label(self.committed_mib());
        match &self.machine {
            Some(machine) if self.overcommitted() => format!(
                "committed {committed} guest RAM of {} · overcommit",
                memory_label(machine.memory_mib)
            ),
            Some(machine) => format!(
                "committed {committed} guest RAM of {}",
                memory_label(machine.memory_mib)
            ),
            None => format!("committed {committed} guest RAM"),
        }
    }

    /// The last line a VM's capture wrote, if it wrote any.
    pub fn latest_line<'a>(&self, vm: &'a crate::api::Vm) -> Option<&'a str> {
        vm.serial
            .as_deref()?
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
    }

    /// The reducer: applies `message` and returns what to do next.
    pub fn apply(&mut self, message: Message) -> Effect {
        if let Some(at_ms) = message.at_ms() {
            self.now_ms = at_ms;
        }
        match message {
            Message::Tick { at_ms } => self.poll_tick(at_ms),
            Message::Runs { at_ms, result } => {
                match result {
                    Ok(page) => {
                        self.absorb(page.runs);
                        self.stream_asked = false;
                        self.activity.retain(&self.runs);
                        self.serial.retain(&self.runs);
                        // A run that left the register takes its rings with it;
                        // the observations already written stay.
                        self.history.retain(&self.runs);
                        // The one place an observation is written: this poll
                        // saw the fleet, so the session's window grows by one.
                        // A failed poll returns further down without sampling,
                        // so unobserved time is never inked.
                        self.history.sample(at_ms, &self.runs, &self.activity);
                        // The session's own export path is stamped where its
                        // first observation landed, once, if the box is empty.
                        self.derive_csv();
                        self.succeeded(at_ms);
                    }
                    Err(error) => {
                        // The register is still owed an answer, but this poll
                        // is over: an event may ask for the next one.
                        self.stream_asked = false;
                        self.failed(at_ms, &error);
                    }
                }
                Effect::none()
            }
            Message::Run { at_ms, id, result } => self.run_answer(at_ms, &id, result),
            Message::Hub { at_ms, result } => self.hub_answer(at_ms, result),
            Message::Machine { at_ms, result } => {
                match result {
                    Ok(machine) => {
                        self.machine = Some(machine);
                        self.succeeded(at_ms);
                    }
                    Err(error) => {
                        self.failed(at_ms, &error);
                        self.notice = Some(Notice {
                            kind: NoticeKind::Failed,
                            text: format!("machine: {error}"),
                        });
                    }
                }
                Effect::none()
            }
            Message::Input {
                at_ms,
                id,
                vm,
                result,
            } => self.console_answer(at_ms, &id, vm, result),
            Message::Key(action) => self.key(action),
            Message::Select { run, vm } => self.select(run, vm),
            Message::SelectVm(vm) => {
                if let Pane::Run(id) = self.pane.clone() {
                    self.selection = Some(Selection { run: id, vm });
                    self.derive_export();
                }
                Effect::none()
            }
            Message::ConsoleEdit(line) => {
                self.console.line = line;
                self.console.walk = None;
                Effect::none()
            }
            Message::ConsoleSend => self.console_send(self.console.line.clone()),
            Message::ConsoleQuick(command) => self.console_send(command.to_string()),
            Message::ConsoleHistory(step) => self.console_history(step),
            Message::Show(pane) => self.show(pane),
            Message::Follow(follow) => {
                self.follow = follow;
                match (&self.pane, follow) {
                    (Pane::Run(id), true) => Effect::fetch(id.clone()),
                    _ => Effect::none(),
                }
            }
            Message::KillClicked(id) => {
                if self.armed.as_deref() == Some(id.as_str()) {
                    self.armed = None;
                    Effect {
                        kill: Some(id),
                        ..Effect::none()
                    }
                } else {
                    self.armed = Some(id);
                    Effect::none()
                }
            }
            Message::Killed { at_ms, id, result } => {
                self.armed = None;
                match result {
                    Ok(run) => {
                        self.succeeded(at_ms);
                        if self.pane == Pane::Run(id.clone()) {
                            self.detail = Some(run);
                        }
                        self.notice = Some(Notice {
                            kind: NoticeKind::Done,
                            text: format!("run {id} killed"),
                        });
                        Effect::refresh(id)
                    }
                    Err(error) => {
                        self.failed(at_ms, &error);
                        self.notice = Some(Notice {
                            kind: NoticeKind::Failed,
                            text: format!("run {id}: {error}"),
                        });
                        Effect::none()
                    }
                }
            }
            Message::SelectAgent(cid) => self.open_agent_console(cid),
            Message::DriveEdit { field, value } => {
                if let Some(console) = self.agent_console.as_mut() {
                    match field {
                        ConsoleField::Tool => console.tool = value,
                        ConsoleField::Args => console.args = value,
                        ConsoleField::Timeout => console.timeout = value,
                    }
                }
                Effect::none()
            }
            Message::DriveSend => self.drive_send(),
            Message::DriveClose => {
                self.agent_console = None;
                self.armed_stop = None;
                Effect::none()
            }
            Message::StopClicked(cid) => {
                if self.armed_stop == Some(cid) {
                    self.armed_stop = None;
                    // The stop's outcome lives in the console, so confirming a
                    // stop opens the console on the agent it names — and marks
                    // the stop in flight — before anything is sent.
                    let effect = self.open_agent_console(cid);
                    debug_assert!(effect.is_idle());
                    if let Some(console) = self.agent_console.as_mut() {
                        console.stop = Some(StopState::Stopping { cid });
                    }
                    Effect::stop(cid)
                } else {
                    self.armed_stop = Some(cid);
                    Effect::none()
                }
            }
            Message::Told {
                cid, name, result, ..
            } => self.told(cid, name, result),
            Message::Stopped { cid, result, .. } => self.stopped(cid, result),
            Message::SpawnClicked => self.spawn_clicked(),
            Message::Spawned { at_ms, result } => match result {
                Ok(run) => {
                    self.succeeded(at_ms);
                    let id = run.id.clone();
                    self.detail = Some(run);
                    self.pane = Pane::Run(id.clone());
                    self.follow = true;
                    self.errors.clear();
                    self.notice = Some(Notice {
                        kind: NoticeKind::Done,
                        text: format!("run {id} created"),
                    });
                    // The create answer is a summary: the serial arrives with
                    // the first follow-up fetch.
                    Effect::refresh(id)
                }
                Err(error) => {
                    self.failed(at_ms, &error);
                    self.notice = Some(Notice {
                        kind: NoticeKind::Failed,
                        text: format!("spawn refused: {error}"),
                    });
                    Effect::none()
                }
            },
            Message::Edit { field, value } => {
                self.form.set(field, value);
                // The field changed, so the last submission's complaint about
                // it no longer describes what is in the box.
                self.errors.retain(|(errored, _)| *errored != field);
                Effect::none()
            }
            Message::Preset(preset) => {
                // A preset only fills the form: it is visible, and adjustable,
                // before anything is sent.
                preset.fill(&mut self.form);
                self.errors.clear();
                Effect::none()
            }
            Message::ExportEdited(path) => {
                self.export.path = path;
                // Emptying the box hands it back to the register, which fills
                // it with the current target's default path again.
                self.export.edited = !self.export.to_write().is_empty();
                if !self.export.edited {
                    self.derive_export();
                }
                Effect::none()
            }
            Message::ExportTarget(target) => {
                self.export.target = target;
                self.derive_export();
                Effect::none()
            }
            Message::Export => self.export_now(),
            Message::Exported {
                at_ms,
                target,
                run,
                vm,
                path,
                result,
            } => {
                let what = match target {
                    ExportTarget::Vm => match vm {
                        Some(vm) => format!("run {run} vm {vm}"),
                        None => format!("run {run}"),
                    },
                    ExportTarget::Run => format!("run {run}, every vm"),
                    ExportTarget::Report => format!("the report of run {run}"),
                };
                match result {
                    Ok(bytes) => {
                        self.succeeded(at_ms);
                        self.notice = Some(Notice {
                            kind: NoticeKind::Done,
                            text: format!("exported {bytes} byte(s) of {what} to {path}"),
                        });
                    }
                    Err(error) => {
                        // The daemon was never asked: this failure is the local
                        // disk's, so it is shown in its own words and the
                        // register's connection stamp is left alone.
                        self.notice = Some(Notice {
                            kind: NoticeKind::Failed,
                            text: format!("export {path}: {error}"),
                        });
                    }
                }
                Effect::none()
            }
            Message::Help(open) => {
                self.help = open;
                Effect::none()
            }
            Message::TelemetryPick(run) => {
                self.telemetry_run = run;
                Effect::none()
            }
            Message::CsvEdited(path) => {
                self.csv.path = path;
                // Emptying the box hands it back to the register, which fills
                // it with the session's default path again.
                self.csv.edited = !self.csv.to_write().is_empty();
                if !self.csv.edited {
                    self.derive_csv();
                }
                Effect::none()
            }
            Message::CsvExport => self.csv_now(),
            Message::CsvExported {
                at_ms,
                path,
                result,
            } => {
                match result {
                    Ok(bytes) => {
                        self.succeeded(at_ms);
                        self.notice = Some(Notice {
                            kind: NoticeKind::Done,
                            text: format!("exported {bytes} byte(s) of the session to {path}"),
                        });
                    }
                    Err(error) => {
                        // The daemon was never asked: this failure is the local
                        // disk's, so it is shown in its own words and the
                        // register's connection stamp is left alone.
                        self.notice = Some(Notice {
                            kind: NoticeKind::Failed,
                            text: format!("export {path}: {error}"),
                        });
                    }
                }
                Effect::none()
            }
            Message::DismissNotice => {
                self.notice = None;
                Effect::none()
            }
            Message::Tasks { at_ms, result } => self.tasks_answer(at_ms, result),
            Message::Task { at_ms, id, result } => self.task_answer(at_ms, &id, result),
            Message::SelectTask(id) => {
                // An artifact read belongs to the task it was asked of:
                // selecting another task drops it rather than leaving a read
                // the detail no longer draws.
                if self
                    .artifact
                    .as_ref()
                    .is_some_and(|state| state.task() != id.as_str())
                {
                    self.artifact = None;
                }
                self.task_selected = Some(id);
                Effect::none()
            }
            Message::TaskRefresh => match self.task_selected.clone() {
                Some(id) => Effect::task(id),
                None => self.refuse("refresh: no task is open on the ledger"),
            },
            Message::TaskEdit { field, value } => {
                self.task_form.set(field, value);
                // The field changed, so the last submission's complaint about
                // it no longer describes what is in the box.
                self.task_errors.retain(|(errored, _)| *errored != field);
                Effect::none()
            }
            Message::TaskCreate => self.task_create(),
            Message::TaskCreated { at_ms, result } => self.task_created(at_ms, result),
            Message::ClaimCidEdit(cid) => {
                self.claim_cid = cid;
                Effect::none()
            }
            Message::TaskClaim => self.task_claim(),
            Message::TaskClaimed {
                at_ms,
                id,
                cid,
                result,
            } => self.task_claimed(at_ms, &id, cid, result),
            Message::FinishNoteEdit(note) => {
                self.finish_note = note;
                Effect::none()
            }
            Message::TaskFinish(outcome) => self.task_finish(outcome),
            Message::TaskFinished {
                at_ms,
                id,
                outcome,
                result,
            } => self.task_finished(at_ms, &id, outcome, result),
            Message::TaskWithdrawClicked(id) => {
                if self.armed_task.as_deref() == Some(id.as_str()) {
                    self.armed_task = None;
                    Effect {
                        withdraw_task: Some(id),
                        ..Effect::none()
                    }
                } else {
                    self.armed_task = Some(id);
                    Effect::none()
                }
            }
            Message::TaskWithdrawn { at_ms, id, result } => {
                self.task_withdrawn(at_ms, &id, result)
            }
            Message::Treasury { at_ms, result } => self.treasury_answer(at_ms, result),
            Message::Agora { at_ms, result } => self.agora_answer(at_ms, result),
            Message::TaskArtifactRead(path) => self.task_artifact_read(path),
            Message::TaskArtifact {
                at_ms,
                id,
                path,
                result,
            } => self.task_artifact_answer(at_ms, &id, &path, result),
            Message::Event { at_ms, event } => self.event(at_ms, event),
        }
    }

    /// The timer fired: poll if the schedule says so.
    ///
    /// Two schedules tick here, because two processes are being read: the
    /// daemon's register, and — when one is configured — the hub's status
    /// surface. They are independent on purpose. A daemon that is down must
    /// not stop the console asking who is connected to a hub that is up, and a
    /// hub that is down must not slow the register's polls or mark its health.
    fn poll_tick(&mut self, at_ms: u64) -> Effect {
        self.now_ms = at_ms;
        let mut effect = self.hub_tick(at_ms);
        // The treasury's own schedule, like the hub's: it ticks here, but
        // independently of the register's cadence and failures.
        effect.poll_treasury = self.treasury_tick(at_ms);
        // And the market's, the same way: its own read, its own failures.
        effect.poll_agora = self.agora_tick(at_ms);
        if at_ms < self.next_poll_at_ms {
            return effect;
        }
        self.polls += 1;
        // The schedule advances when the call goes out, and again when the
        // answer lands (with the backoff of however many have failed since).
        self.next_poll_at_ms = at_ms.saturating_add(poll_interval_ms(self.refresh_ms, self.failures));
        effect.poll_list = true;
        // Everything on the sheet that moves is refreshed by one tick: the
        // register always, the run pane's serial while it follows, the
        // machine's uptime while it is on the sheet, and the map's pulse
        // (which needs every run's serial to have anything to measure).
        match &self.pane {
            Pane::Run(id) if self.follow => effect.fetch_run = Some(id.clone()),
            Pane::Machine => effect.poll_machine = true,
            // The ledger is one read — every task, oldest first — and the task
            // events patch its rows in between. It draws no substrate and
            // decomposes no serial, so nothing else is asked for.
            Pane::Ledger => effect.poll_tasks = true,
            Pane::Map => {
                effect.poll_machine = true;
                effect.fetch_map = self.run_ids();
            }
            // The session's charts are per-run decompositions of serial
            // throughput, so the telemetry pane watches every run's serial the
            // same way the map's pulse does, and takes the machine's total for
            // the RAM chart's ceiling.
            Pane::Telemetry => {
                effect.poll_machine = true;
                effect.fetch_map = self.run_ids();
            }
            _ => {}
        }
        effect
    }

    /// The hub's half of the tick: `GET /status`, on the hub's own schedule.
    ///
    /// The status surface is off unless the host was started with one, so a
    /// model with no hub has nothing to schedule and this is exactly the old
    /// tick. With one, the first tick reads it (the schedule starts due) and
    /// the cadence is the register's, backing off on the hub's own failures.
    fn hub_tick(&mut self, at_ms: u64) -> Effect {
        if self.hub.is_none() || at_ms < self.hub_next_at_ms {
            return Effect::none();
        }
        self.hub_next_at_ms = at_ms.saturating_add(poll_interval_ms(self.refresh_ms, self.hub_failures));
        Effect::hub()
    }

    /// The hub's answer: the connected half of the roster, or why it is not
    /// there.
    ///
    /// The hub's health is its own — a second process on a second port — so
    /// this never touches the register's failures, its schedule, or what
    /// [`Model::health`] reports. A hub that refuses is a hub the pane says is
    /// unreachable, not a daemon that reads down.
    fn hub_answer(&mut self, at_ms: u64, result: Result<HubStatus, ApiError>) -> Effect {
        match result {
            Ok(status) => {
                self.hub_status = Some(status);
                self.hub_error = None;
                self.hub_last_ok_ms = Some(at_ms);
                self.hub_failures = 0;
                // Like the register's: the cadence measures from the answer,
                // not from the request.
                self.hub_next_at_ms = at_ms.saturating_add(self.refresh_ms);
            }
            Err(error) => {
                // A connection list is a statement about now: an answer that
                // did not arrive leaves the pane with nothing to claim.
                self.hub_status = None;
                self.hub_error = Some(error.to_string());
                self.hub_failures = self.hub_failures.saturating_add(1);
                self.hub_next_at_ms = at_ms
                    .saturating_add(poll_interval_ms(self.refresh_ms, self.hub_failures));
            }
        }
        Effect::none()
    }

    /// Open the roster's operator console aimed at `cid`.
    ///
    /// An already open console on the same agent is kept — the form the
    /// operator typed is not thrown away by a second press — while another
    /// agent's console is replaced.
    fn open_agent_console(&mut self, cid: u64) -> Effect {
        if self.agent_console.as_ref().map(|console| console.cid) != Some(cid) {
            self.agent_console = Some(RosterConsole::new(cid));
        }
        Effect::none()
    }

    /// The drive form's send button: validate against the hub's own contract
    /// and, when it holds, ask for the call. A form that does not hold is
    /// recorded as [`DriveState::Rejected`] — the hub's own wording — and
    /// nothing is sent, so nothing reaches the agent on a bad form.
    fn drive_send(&mut self) -> Effect {
        let Some(console) = self.agent_console.as_ref() else {
            return Effect::none();
        };
        let cid = console.cid;
        let name = console.tool.trim().to_string();
        let args_text = console.args.clone();
        let timeout_text = console.timeout.trim().to_string();

        if name.is_empty() {
            return self.reject_drive(cid, "a tell needs a tool name");
        }
        if name.len() > MAX_TOOL_NAME {
            return self.reject_drive(
                cid,
                &format!("the tool name is longer than {MAX_TOOL_NAME} bytes"),
            );
        }
        let args = match parse_args(&args_text) {
            Ok(args) => args,
            Err(message) => return self.reject_drive(cid, &message),
        };
        let timeout_ms = match timeout_text.parse::<u64>() {
            Ok(ms) if (1..=MAX_TELL_TIMEOUT_MS).contains(&ms) => ms,
            _ => {
                return self
                    .reject_drive(cid, &format!("timeout_ms must be 1..={MAX_TELL_TIMEOUT_MS}"));
            }
        };
        if let Some(console) = self.agent_console.as_mut() {
            console.drive = Some(DriveState::Driving {
                cid,
                name: name.clone(),
                timeout_ms,
            });
        }
        Effect::tell(TellCall {
            cid,
            name,
            args,
            timeout_ms,
        })
    }

    /// Record a drive the form itself refused, and send nothing.
    fn reject_drive(&mut self, cid: u64, message: &str) -> Effect {
        if let Some(console) = self.agent_console.as_mut() {
            console.drive = Some(DriveState::Rejected {
                cid,
                message: message.to_string(),
            });
        }
        Effect::none()
    }

    /// A `POST /tell` answered: the agent's own answer, or the hub's own words
    /// for a refusal.
    ///
    /// It lands on the console it was asked from when that console is still
    /// open on the same agent; when it is not — the console moved or closed
    /// while the call was in flight — a one-line notice carries the outcome, so
    /// an answer that arrived is never silently lost. A hub refusal never
    /// touches the register's health: the hub is a second process.
    fn told(&mut self, cid: u64, name: String, result: Result<TellAnswer, ApiError>) -> Effect {
        let (state, kind) = match result {
            Ok(answer) => {
                let kind = if answer.ok {
                    NoticeKind::Done
                } else {
                    // The call finished; the tool declined. Not a register
                    // failure, but worth an amber word when it falls to a
                    // notice.
                    NoticeKind::Failed
                };
                (
                    DriveState::Answered {
                        cid,
                        name: name.clone(),
                        ok: answer.ok,
                        output: answer.output,
                    },
                    kind,
                )
            }
            Err(error) => {
                let state = drive_error(cid, name.clone(), error);
                (state, NoticeKind::Failed)
            }
        };
        match self.agent_console.as_mut() {
            Some(console) if console.cid == cid => console.drive = Some(state),
            _ => {
                let text = state
                    .text()
                    .map(|text| format!("cid {cid} · {text}"))
                    .unwrap_or_else(|| format!("cid {cid} · {}", state.mark().to_lowercase()));
                self.notice = Some(Notice { kind, text });
            }
        }
        Effect::none()
    }

    /// A `POST /stop` answered: the session ended, or it did not — and, every
    /// time it did not, that nothing was torn down.
    fn stopped(&mut self, cid: u64, result: Result<StopOutcome, ApiError>) -> Effect {
        let (state, kind) = match result {
            Ok(StopOutcome::Stopped { calls }) => (
                StopState::Stopped { cid, calls },
                NoticeKind::Done,
            ),
            Ok(StopOutcome::StillUp { reason }) => {
                (StopState::StillUp { cid, reason }, NoticeKind::Failed)
            }
            Err(error) => (stop_error(cid, error), NoticeKind::Failed),
        };
        match self.agent_console.as_mut() {
            Some(console) if console.cid == cid => console.stop = Some(state),
            _ => {
                self.notice = Some(Notice {
                    kind,
                    text: format!("cid {cid} · {}", state.text()),
                });
            }
        }
        Effect::none()
    }

    /// The treasury's own schedule, like the hub's.
    ///
    /// The pane's read is the treasury and its journal at once, on a cadence
    /// of its own: it is not the register's poll, and — because the endpoint
    /// is one an older daemon may not serve — a treasury that cannot be read
    /// never marks the register down or slows its polls. The pane only reads
    /// while it is showing; the treasury is not read for a sheet nobody has
    /// opened.
    fn treasury_tick(&mut self, at_ms: u64) -> bool {
        if self.pane != Pane::Treasury || at_ms < self.treasury_next_at_ms {
            return false;
        }
        self.treasury_next_at_ms = at_ms
            .saturating_add(poll_interval_ms(self.refresh_ms, self.treasury_failures));
        true
    }

    /// The treasury answered — or could not be read.
    ///
    /// The read is kept when a later one fails: the journal is append-only, so
    /// what the console holds stays true, and the failure is recorded beside
    /// it rather than pretending the treasury is empty. Its health is its own:
    /// a second surface, like the hub's, that never moves the register's.
    fn treasury_answer(&mut self, at_ms: u64, result: Result<TreasuryRead, ApiError>) -> Effect {
        self.treasury_asked = false;
        match result {
            Ok(read) => {
                self.treasury = Some(read);
                self.treasury_error = None;
                self.treasury_last_ok_ms = Some(at_ms);
                self.treasury_failures = 0;
                self.treasury_next_at_ms = at_ms.saturating_add(self.refresh_ms);
            }
            Err(error) => {
                self.treasury_error = Some(error.to_string());
                self.treasury_failures = self.treasury_failures.saturating_add(1);
                self.treasury_next_at_ms = at_ms
                    .saturating_add(poll_interval_ms(self.refresh_ms, self.treasury_failures));
            }
        }
        Effect::none()
    }

    /// Applies one treasury event: the journal's own delta between its reads.
    ///
    /// A frame announces that the journal grew and names the entry — its kind
    /// and size and nothing else. It carries no memo, no postings and no
    /// clock, so the console never assembles an entry from one: a frame for an
    /// entry the console already holds is one it has read (a duplicate, or an
    /// older entry replayed) and is a no-op, while a frame past the newest
    /// entry it holds means the journal moved past what it saw, and it reads
    /// the journal again — once, however many entries arrive at once.
    fn treasury_event(&mut self, entry: u64) -> Effect {
        // Only a pane that is showing is refreshed: nothing else draws the
        // treasury, and the sheet reads it when it opens.
        if self.pane != Pane::Treasury {
            return Effect::none();
        }
        let newest = self
            .treasury
            .as_ref()
            .and_then(|read| read.journal.entries.last())
            .map(|held| held.id);
        match newest {
            Some(newest) if entry <= newest => Effect::none(),
            _ => self.ask_for_the_treasury(),
        }
    }

    /// Asks for the treasury, at most once per outstanding answer.
    fn ask_for_the_treasury(&mut self) -> Effect {
        if self.treasury_asked {
            return Effect::none();
        }
        self.treasury_asked = true;
        Effect::treasury()
    }

    /// The market's own schedule: read the board only while it is showing.
    ///
    /// Like the treasury's, this is independent of the register's cadence and
    /// failures — it returns `false` off the sheet, and backs off on the
    /// market's own failures — so a daemon that serves the register but not
    /// the agora never reads the register down, and a console that never
    /// opened the sheet never reads the board at all.
    fn agora_tick(&mut self, at_ms: u64) -> bool {
        if self.pane != Pane::Market || at_ms < self.agora_next_at_ms {
            return false;
        }
        self.agora_next_at_ms = at_ms
            .saturating_add(poll_interval_ms(self.refresh_ms, self.agora_failures));
        true
    }

    /// The market answered — or could not be read.
    ///
    /// The read is kept when a later one fails: a bid the daemon has already
    /// recorded is a fact, so what the console holds stays true, and the
    /// failure is recorded beside it rather than pretending the board is
    /// empty. Its health is its own: a second surface, like the hub's and the
    /// treasury's, that never moves the register's.
    fn agora_answer(&mut self, at_ms: u64, result: Result<Board, ApiError>) -> Effect {
        self.agora_asked = false;
        match result {
            Ok(board) => {
                self.agora = Some(board);
                self.agora_error = None;
                self.agora_last_ok_ms = Some(at_ms);
                self.agora_failures = 0;
                self.agora_next_at_ms = at_ms.saturating_add(self.refresh_ms);
            }
            Err(error) => {
                self.agora_error = Some(error.to_string());
                self.agora_failures = self.agora_failures.saturating_add(1);
                self.agora_next_at_ms = at_ms
                    .saturating_add(poll_interval_ms(self.refresh_ms, self.agora_failures));
            }
        }
        Effect::none()
    }

    /// Applies one market event: the board's own delta between its reads.
    ///
    /// A frame announces that a call's market moved and names the call, what
    /// moved it, the agent and the price — and nothing else. It carries no
    /// bidder, no escrow entry and no clock, so the console never assembles a
    /// bid or a call from one: a frame the board already reflects (a bid the
    /// call holds from that agent at that price, or the award it already
    /// holds) is a duplicate and a no-op, while anything else means the board
    /// moved past what the pane saw, and it reads the board again — once,
    /// however many bids arrive at once.
    fn agora_event(&mut self, task: &str, kind: AgoraKind, agent_cid: u64, price: u64) -> Effect {
        // Only a pane that is showing is refreshed: nothing else draws the
        // board, and the sheet reads it when it opens.
        if self.pane != Pane::Market {
            return Effect::none();
        }
        // A frame the pane already holds is one it has read: nothing moved.
        if let Some(board) = &self.agora {
            if let Some(call) = board.calls.iter().find(|call| call.task == task) {
                let seen = match kind {
                    AgoraKind::Bid => call
                        .bids
                        .iter()
                        .any(|bid| bid.agent_cid == agent_cid && bid.price == price),
                    AgoraKind::Award => call.award.get().is_some_and(|award| {
                        award.agent_cid == agent_cid && award.price == price
                    }),
                    AgoraKind::Unknown => false,
                };
                if seen {
                    return Effect::none();
                }
            }
        }
        self.ask_for_the_market()
    }

    /// Asks for the market, at most once per outstanding answer.
    fn ask_for_the_market(&mut self) -> Effect {
        if self.agora_asked {
            return Effect::none();
        }
        self.agora_asked = true;
        Effect::agora()
    }

    /// Every run in the register, by id: what the map's pulse is fed from.
    fn run_ids(&self) -> Vec<String> {
        self.runs.iter().map(|run| run.id.clone()).collect()
    }

    /// The stream answered: an event is the daemon talking, so it keeps the
    /// register's clock and the age of its last answer current.
    ///
    /// It deliberately does not touch the poll's schedule or its failures: a
    /// daemon that streams happily while refusing its polls still reads down,
    /// and a stream that has gone quiet never makes a healthy daemon look
    /// down. The stream accelerates; the poll is still the truth.
    fn heard(&mut self, at_ms: u64) {
        self.last_ok_ms = Some(at_ms);
    }

    /// Applies one event from `GET /api/v0/events`.
    ///
    /// The daemon's bus has no backlog, so an event that names a run the
    /// register has not read yet is not an event to build a row from — the
    /// frame carries an id and a state and nothing else, and the register
    /// does not invent vcpu counts or memory. It asks for the register
    /// instead, once, and the answer lands in about as long as a poll takes.
    ///
    /// The same is true of the other direction: an event that says a run the
    /// register knows *ended* is live again. The same run never comes back —
    /// either the event was written before the answer that settled it, or the
    /// daemon restarted and began its run ids again. The register's answer,
    /// with every run's creation time in it, is what tells those apart.
    fn event(&mut self, at_ms: u64, event: Event) -> Effect {
        self.heard(at_ms);
        match event {
            Event::Run { id, state } => {
                if !self.registered(&id) {
                    return self.ask_for_the_register();
                }
                if self.run_is_ended(&id) && !run_ended(state) {
                    return self.ask_for_the_register();
                }
                self.move_run_state(&id, state);
                Effect::none()
            }
            Event::Vm { run, vm, state } => {
                if !self.registered(&run) {
                    return self.ask_for_the_register();
                }
                if self.vm_is_stopped(&run, vm) && !vm_stopped(state) {
                    return self.ask_for_the_register();
                }
                self.move_vm_state(&run, vm, state);
                Effect::none()
            }
            Event::Serial {
                run,
                vm,
                offset,
                text,
            } => {
                if !self.registered(&run) {
                    return self.ask_for_the_register();
                }
                self.merge_serial(at_ms, &run, vm, offset, &text)
            }
            Event::Task {
                id,
                state,
                assignee,
            } => self.task_event(&id, state, assignee),
            Event::Treasury { entry, .. } => self.treasury_event(entry),
            Event::Agora {
                task,
                kind,
                agent_cid,
                price,
            } => self.agora_event(&task, kind, agent_cid, price),
        }
    }

    /// Applies one task event: the ledger's own deltas between its polls.
    ///
    /// The bus has no backlog, so an event for a task the ledger has not read
    /// carries an id, a state and an assignee and nothing else — no goal, no
    /// budget and no clock. The ledger does not invent a row from it: it asks
    /// for the ledger instead, once, and the answer lands in about as long as a
    /// poll takes.
    ///
    /// A settled task never moves again, so an event that would walk one back
    /// is not applied at all: either it was written before the answer that
    /// settled the task, or the daemon restarted and began its ids again. What
    /// a frame does not carry — the goal, the budget, the times — is left
    /// exactly as the poll wrote it, because a task's history is not something
    /// a delta may invent.
    fn task_event(&mut self, id: &str, state: TaskState, assignee: Option<String>) -> Effect {
        let Some(held) = self.tasks.iter_mut().find(|task| task.id == id) else {
            return self.ask_for_the_ledger();
        };
        if task_ended(held.state) && !task_ended(state) {
            return Effect::none();
        }
        held.state = state;
        held.assignee_id = Column::filled(assignee);
        Effect::none()
    }

    /// Asks for the ledger, at most once per outstanding answer.
    fn ask_for_the_ledger(&mut self) -> Effect {
        if self.ledger_asked {
            return Effect::none();
        }
        self.ledger_asked = true;
        Effect::ledger()
    }

    /// Whether the register has read this run at all.
    fn registered(&self, id: &str) -> bool {
        self.runs.iter().any(|run| run.id == id)
    }

    /// Asks for the register, at most once per outstanding answer.
    fn ask_for_the_register(&mut self) -> Effect {
        if self.stream_asked {
            return Effect::none();
        }
        self.stream_asked = true;
        Effect::poll()
    }

    /// Takes a list answer into the register.
    ///
    /// The daemon's runs only ever move live to terminal, so a state the
    /// register already knows has ended is never walked back by a snapshot:
    /// an answer read before the transition, or an event that arrived after
    /// the answer it was written before, describes an older fleet, and the
    /// register keeps what it saw.
    ///
    /// A run is identified by its id *and* when it was created. A daemon that
    /// restarts begins its run ids again, so `r1` after a restart is a
    /// different run wearing a reused id: nothing the register remembered
    /// about the old one — its settled state, the length of its serial
    /// capture, its rates — applies to it.
    fn absorb(&mut self, mut page: Vec<Run>) {
        let mut restarted: Vec<String> = Vec::new();
        for run in page.iter_mut() {
            let Some(known) = self.runs.iter().find(|known| known.id == run.id) else {
                continue;
            };
            if known.created_at != run.created_at {
                restarted.push(run.id.clone());
                continue;
            }
            run.state = moved_run_state(known.state, run.state);
            if run.ended_at.is_none() {
                run.ended_at = known.ended_at;
            }
            for vm in run.vms.iter_mut() {
                if let Some(slot) = known.vms.iter().find(|slot| slot.id == vm.id) {
                    vm.state = moved_vm_state(slot.state, vm.state);
                }
            }
        }
        self.runs = page;
        for id in &restarted {
            self.serial.forget(id);
            self.activity.forget(id);
            self.history.forget(id);
        }
    }

    /// Whether the register's own row for this run is one that ended.
    fn run_is_ended(&self, id: &str) -> bool {
        self.runs
            .iter()
            .any(|run| run.id == id && run_ended(run.state))
    }

    /// Whether the register's own row for this VM is one that stopped.
    fn vm_is_stopped(&self, id: &str, vm: u32) -> bool {
        self.runs.iter().any(|run| {
            run.id == id
                && run
                    .vms
                    .iter()
                    .any(|slot| slot.id == vm && vm_stopped(slot.state))
        })
    }

    /// Moves a run's state, where the register and the open run both read it.
    fn move_run_state(&mut self, id: &str, state: RunState) {
        for run in self.runs.iter_mut().filter(|run| run.id == id) {
            run.state = moved_run_state(run.state, state);
        }
        if let Some(detail) = self.detail.as_mut().filter(|run| run.id == id) {
            detail.state = moved_run_state(detail.state, state);
        }
    }

    /// Moves one VM's state, in the register and in the open run.
    fn move_vm_state(&mut self, id: &str, vm: u32, state: VmState) {
        if let Some(found) = self.runs.iter_mut().find(|run| run.id == id) {
            if let Some(slot) = found.vms.iter_mut().find(|slot| slot.id == vm) {
                slot.state = moved_vm_state(slot.state, state);
            }
        }
        if let Some(found) = self.detail.as_mut().filter(|detail| detail.id == id) {
            if let Some(slot) = found.vms.iter_mut().find(|slot| slot.id == vm) {
                slot.state = moved_vm_state(slot.state, state);
            }
        }
    }

    /// Merges one serial event: into the register's transcript when it holds
    /// that capture, and into the activity feed either way.
    ///
    /// The merge itself is [`Feed`]'s, so a replayed or clipped event appends
    /// nothing rather than duplicating bytes. Only a gap asks for anything:
    /// the capture the register holds is no longer contiguous with the
    /// stream, so it is read again rather than filled in.
    fn merge_serial(&mut self, at_ms: u64, run: &str, vm: u32, offset: u64, text: &str) -> Effect {
        match self.serial.merge(run, vm, offset, text) {
            Merge::Known => Effect::none(),
            Merge::Append { skip, .. } => {
                if let Some(capture) = self.held_capture(run, vm) {
                    if let Some(tail) = text.get(skip..) {
                        capture.push_str(tail);
                    }
                }
                // The rate between polls: the map's pulse, the run pane's
                // sparkline and the telemetry pane all read it.
                if let Some(lines) = self.serial.lines(run, vm) {
                    let lps = self
                        .activity
                        .observe(run, vm, usize::try_from(lines).unwrap_or(usize::MAX), at_ms);
                    self.history.observe_vm(run, vm, lps);
                }
                Effect::none()
            }
            Merge::Gap { .. } if self.holds_capture(run, vm) => Effect::fetch(run),
            Merge::Gap { .. } => Effect::none(),
        }
    }

    /// The capture the register is holding for one VM: the open run's view is
    /// the only one that carries captures (the list endpoint sends none).
    fn held_capture(&mut self, run: &str, vm: u32) -> Option<&mut String> {
        self.detail
            .as_mut()
            .filter(|detail| detail.id == run)?
            .vms
            .iter_mut()
            .find(|slot| slot.id == vm)?
            .serial
            .as_mut()
    }

    /// Whether the register is showing that VM at all, and so whether a gap
    /// is worth reading the run again for.
    fn holds_capture(&self, run: &str, vm: u32) -> bool {
        self.detail.as_ref().is_some_and(|detail| {
            detail.id == run && detail.vms.iter().any(|slot| slot.id == vm)
        })
    }

    /// A single-run answer, which also tells the register how to read the run.
    fn run_answer(&mut self, at_ms: u64, id: &str, result: Result<Run, ApiError>) -> Effect {
        match result {
            Ok(run) => {
                self.succeeded(at_ms);
                self.observe(&run);
                if self.pane == Pane::Run(id.to_string()) {
                    self.detail = Some(run);
                }
                Effect::none()
            }
            // A 404 is an answer: the daemon is alive, the run is gone.
            Err(error) if error.is_not_found() => {
                self.succeeded(at_ms);
                if self.pane == Pane::Run(id.to_string()) {
                    self.pane = Pane::Fleet;
                    self.detail = None;
                    self.armed = None;
                }
                self.notice = Some(Notice {
                    kind: NoticeKind::Failed,
                    text: format!("run {id} is gone"),
                });
                Effect::none()
            }
            Err(error) => {
                self.failed(at_ms, &error);
                Effect::none()
            }
        }
    }

    /// The ledger answered: every task, oldest first.
    fn tasks_answer(&mut self, at_ms: u64, result: Result<TasksPage, ApiError>) -> Effect {
        self.ledger_asked = false;
        match result {
            Ok(page) => {
                self.ledger_read = true;
                self.absorb_tasks(page.tasks);
                self.succeeded(at_ms);
            }
            Err(error) => self.failed(at_ms, &error),
        }
        Effect::none()
    }

    /// Takes a ledger answer into the ledger.
    ///
    /// A task only ever moves forward, so a row the ledger already holds keeps
    /// the state it settled into: an answer read before a transition the events
    /// already delivered describes an older ledger, and one that would walk a
    /// settled task back is not the newer word.
    ///
    /// A task is identified by its id *and* when it was created — a daemon that
    /// restarts begins its task ids again — so a row wearing a reused id
    /// inherits nothing from the task before it, and a selection that named one
    /// of those rows is dropped rather than drawn from another daemon's life.
    fn absorb_tasks(&mut self, mut page: Vec<Task>) {
        for task in page.iter_mut() {
            let Some(held) = self.tasks.iter().find(|held| held.id == task.id) else {
                continue;
            };
            if held.created_at != task.created_at {
                continue;
            }
            let settled = moved_task_state(held.state, task.state);
            if settled != task.state {
                task.state = settled;
                task.assignee_id = held.assignee_id.clone();
            }
        }
        let dropped = match self.task_selected.as_deref() {
            None => false,
            Some(id) => match self.tasks.iter().find(|task| task.id == id) {
                Some(held) => !page
                    .iter()
                    .any(|task| task.id == id && task.created_at == held.created_at),
                None => true,
            },
        };
        if dropped {
            self.task_selected = None;
            self.armed_task = None;
            self.artifact = None;
        }
        self.tasks = page;
    }

    /// One task's own answer: the ledger's refresh reads a row again.
    fn task_answer(&mut self, at_ms: u64, id: &str, result: Result<Task, ApiError>) -> Effect {
        match result {
            Ok(task) => {
                self.succeeded(at_ms);
                self.file_task(task);
                Effect::none()
            }
            // A 404 is an answer: the daemon is alive, the task is gone.
            Err(error) if error.is_not_found() => {
                self.succeeded(at_ms);
                if self.task_selected.as_deref() == Some(id) {
                    self.task_selected = None;
                    self.armed_task = None;
                    self.artifact = None;
                }
                self.refuse(format!("task {id} is gone"))
            }
            Err(error) => {
                self.failed(at_ms, &error);
                self.refuse(format!("task {id}: {error}"))
            }
        }
    }

    /// Files one task the daemon itself reported — a create's answer, a
    /// single-task read, a claim's, a finish's or a withdraw's: in place when
    /// the ledger holds its id, at the end when it does not.
    ///
    /// Only the daemon's own rows are filed here. A *task event* never reaches
    /// this: a frame carries an id and a state and nothing else, so it moves a
    /// row the ledger already holds rather than standing in for one.
    fn file_task(&mut self, task: Task) {
        match self.tasks.iter_mut().find(|held| held.id == task.id) {
            Some(held) => *held = task,
            None => self.tasks.push(task),
        }
    }

    /// Submits the ledger's create form: valid means send, invalid means say
    /// what is wrong and send nothing.
    fn task_create(&mut self) -> Effect {
        match self.task_form.validate() {
            Ok(create) => {
                self.task_errors.clear();
                Effect {
                    create_task: Some(create),
                    ..Effect::none()
                }
            }
            Err(errors) => {
                let why = format!("the form has {} problem(s)", errors.len());
                self.task_errors = errors;
                self.refuse(why)
            }
        }
    }

    /// A create's answer: the task the daemon filed, in its first state.
    fn task_created(&mut self, at_ms: u64, result: Result<Task, ApiError>) -> Effect {
        match result {
            Ok(task) => {
                self.succeeded(at_ms);
                let id = task.id.clone();
                self.task_errors.clear();
                self.file_task(task);
                self.task_selected = Some(id.clone());
                self.notice = Some(Notice {
                    kind: NoticeKind::Done,
                    text: format!("task {id} created"),
                });
                // The ledger's own answer carries the new task in the order
                // the daemon keeps it, alongside every task the console has
                // not read yet.
                Effect::ledger()
            }
            Err(error) => {
                self.failed(at_ms, &error);
                self.refuse(format!("task refused: {error}"))
            }
        }
    }

    /// Claims the open task as the agent whose guest CID is in the claim box.
    ///
    /// The two guards are the daemon's own: only an open task can be claimed,
    /// and only a whole number names an agent. A refusal here is the ledger
    /// saying so in the notice line — and a refused claim is still the
    /// daemon's refusal to make: the 409 it answers with names the CID no live
    /// agent holds, in its own words.
    fn task_claim(&mut self) -> Effect {
        let Some((id, state)) = self
            .selected_task()
            .map(|task| (task.id.clone(), task.state))
        else {
            return self.refuse("claim: no task is open on the ledger");
        };
        if !state.is_claimable() {
            return self.refuse(format!(
                "task {id} is {}: only an open task can be claimed",
                state.label()
            ));
        }
        match crate::ledger::agent_cid(&self.claim_cid) {
            Ok(agent_cid) => Effect {
                claim_task: Some(ClaimCall { id, agent_cid }),
                ..Effect::none()
            },
            Err(why) => self.refuse(why),
        }
    }

    /// A claim's answer: the task, bound to the agent that claimed it.
    fn task_claimed(
        &mut self,
        at_ms: u64,
        id: &str,
        cid: u64,
        result: Result<Task, ApiError>,
    ) -> Effect {
        match result {
            Ok(task) => {
                self.succeeded(at_ms);
                self.file_task(task);
                self.notice = Some(Notice {
                    kind: NoticeKind::Done,
                    text: format!("task {id} claimed by cid {cid}"),
                });
                Effect::none()
            }
            Err(error) => {
                self.failed(at_ms, &error);
                self.refuse(format!("task {id} claim: {error}"))
            }
        }
    }

    /// Finishes the claimed task the ledger has open, as `done` or `failed`.
    fn task_finish(&mut self, outcome: FinishOutcome) -> Effect {
        let Some((id, state)) = self
            .selected_task()
            .map(|task| (task.id.clone(), task.state))
        else {
            return self.refuse("finish: no task is open on the ledger");
        };
        if !state.is_finishable() {
            return self.refuse(format!(
                "task {id} is {}: only a claimed task can be finished",
                state.label()
            ));
        }
        match crate::ledger::finish_note(&self.finish_note) {
            Ok(note) => Effect {
                finish_task: Some(FinishCall { id, outcome, note }),
                ..Effect::none()
            },
            Err(why) => self.refuse(why),
        }
    }

    /// A finish's answer: the task in the state the agent reported.
    fn task_finished(
        &mut self,
        at_ms: u64,
        id: &str,
        outcome: FinishOutcome,
        result: Result<Task, ApiError>,
    ) -> Effect {
        match result {
            Ok(task) => {
                self.succeeded(at_ms);
                self.file_task(task);
                // The note described this finish, and it is on the task now.
                self.finish_note.clear();
                self.notice = Some(Notice {
                    kind: NoticeKind::Done,
                    text: format!("task {id} finished {}", outcome.label()),
                });
                Effect::none()
            }
            Err(error) => {
                self.failed(at_ms, &error);
                self.refuse(format!("task {id} finish: {error}"))
            }
        }
    }

    /// A withdraw's answer: the task, given up before it finished.
    fn task_withdrawn(&mut self, at_ms: u64, id: &str, result: Result<Task, ApiError>) -> Effect {
        self.armed_task = None;
        match result {
            Ok(task) => {
                self.succeeded(at_ms);
                self.file_task(task);
                self.notice = Some(Notice {
                    kind: NoticeKind::Done,
                    text: format!("task {id} withdrawn"),
                });
                Effect::none()
            }
            Err(error) => {
                self.failed(at_ms, &error);
                self.refuse(format!("task {id} withdraw: {error}"))
            }
        }
    }

    /// Reads one of the open task's artifacts: the path its note names, read
    /// through the task's own agent.
    ///
    /// The row that was pressed only exists under an open task's detail, so
    /// the task is the ledger's selection. The read is in flight from here
    /// until its answer lands.
    fn task_artifact_read(&mut self, path: String) -> Effect {
        let Some(task) = self.task_selected.clone() else {
            return self.refuse("artifact: no task is open on the ledger");
        };
        self.artifact = Some(ArtifactState::Loading {
            task: task.clone(),
            path: path.clone(),
        });
        Effect::artifact(task, path)
    }

    /// An artifact read's answer: the file the task's agent read back out of
    /// its workspace, or the reason it did not.
    ///
    /// This call's whole purpose is a read the agent can refuse — a path
    /// outside its workspace, a directory, a file that is not text — so a
    /// refusal *is* the daemon answering rather than failing: it is drawn in
    /// the artifact section, in the daemon's own words, and it neither backs
    /// the poll off nor marks the register down. Only a daemon that could not
    /// be reached is a failed call.
    ///
    /// A read answers for the task it was asked of: one for a task the ledger
    /// no longer has open is a view the operator has left, and is dropped
    /// rather than drawn under another task.
    fn task_artifact_answer(
        &mut self,
        at_ms: u64,
        id: &str,
        path: &str,
        result: Result<TaskArtifact, ApiError>,
    ) -> Effect {
        if self.task_selected.as_deref() != Some(id) {
            return Effect::none();
        }
        match &result {
            Err(error) if error.is_unreachable() => self.failed(at_ms, error),
            _ => self.succeeded(at_ms),
        }
        self.artifact = Some(ArtifactState::from_answer(id, path, result));
        Effect::none()
    }

    /// Refuses locally: a complaint in the notice line, and nothing sent.
    ///
    /// The notice line is where every refusal already lands — the daemon's own
    /// words included — so a guard that never reached the daemon reads exactly
    /// as one that did.
    fn refuse(&mut self, why: impl Into<String>) -> Effect {
        self.notice = Some(Notice {
            kind: NoticeKind::Failed,
            text: why.into(),
        });
        Effect::none()
    }

    /// Shows `pane`, fetching what it needs.
    fn show(&mut self, pane: Pane) -> Effect {
        // Arming is per sheet: a kill or a withdraw that was armed on the
        // sheet the operator just left is not armed on the one arriving. The
        // roster's operator console is the roster's, so leaving the roster
        // closes it too — and disarms any stop waiting on a second press.
        self.armed = None;
        self.armed_task = None;
        if pane != Pane::Roster {
            self.agent_console = None;
            self.armed_stop = None;
        }
        match pane {
            Pane::Fleet => {
                self.pane = Pane::Fleet;
                self.detail = None;
                Effect::poll()
            }
            Pane::Run(id) => {
                if self.detail.as_ref().map(|run| run.id.as_str()) != Some(id.as_str()) {
                    self.detail = None;
                }
                self.selection = match &self.selection {
                    Some(selection) if selection.run == id => Some(selection.clone()),
                    _ => Some(Selection {
                        run: id.clone(),
                        vm: 0,
                    }),
                };
                self.pane = Pane::Run(id.clone());
                self.follow = true;
                self.derive_export();
                Effect::fetch(id)
            }
            Pane::Spawn => {
                self.pane = Pane::Spawn;
                self.detail = None;
                self.errors.clear();
                Effect::none()
            }
            Pane::Machine => {
                self.pane = Pane::Machine;
                self.detail = None;
                Effect::machine()
            }
            Pane::Map => {
                self.pane = Pane::Map;
                self.detail = None;
                // The plate is labelled with the machine's model and its pulse
                // is measured from every run's serial, so opening it asks for
                // both at once.
                Effect::map(self.run_ids())
            }
            Pane::Roster => {
                self.pane = Pane::Roster;
                self.detail = None;
                // One row per role agent, each with its own rate: the register
                // and every run's serial, the same read the map's pulse is
                // measured from. The roster draws no substrate, so the machine
                // is not asked for. With a hub configured, its status surface
                // is read at once too: the pane's other half is a live
                // connection list, and opening the sheet is a reason to want it
                // now rather than at the next tick.
                let mut effect = Effect::roster(self.run_ids());
                effect.poll_hub = self.has_hub();
                effect
            }
            Pane::Ledger => {
                self.pane = Pane::Ledger;
                self.detail = None;
                // The ledger is one read: every task, oldest first, each with
                // its own facts. Opening the sheet is a reason to want them now
                // rather than at the next tick, and the task events patch rows
                // in between.
                Effect::ledger()
            }
            Pane::Treasury => {
                self.pane = Pane::Treasury;
                self.detail = None;
                // The treasury is one read too: the accounts with their totals
                // and the journal at once. Opening the sheet is a reason to
                // want them now, and the schedule then runs from this read so
                // the next tick does not read again on top of it.
                self.treasury_next_at_ms =
                    self.now_ms.saturating_add(self.refresh_ms);
                Effect::treasury()
            }
            Pane::Market => {
                self.pane = Pane::Market;
                self.detail = None;
                // The market is one read as well: the whole board, every call
                // with its bids and its award. Opening the sheet is a reason to
                // want it now, and the schedule then runs from this read so the
                // next tick does not read again on top of it.
                self.agora_next_at_ms = self.now_ms.saturating_add(self.refresh_ms);
                Effect::agora()
            }
            Pane::Telemetry => {
                self.pane = Pane::Telemetry;
                self.detail = None;
                // The session's figures come from the register's own answers:
                // the list poll, every run's serial for the decomposition, and
                // the machine's total for the RAM chart's ceiling.
                self.derive_csv();
                Effect::map(self.run_ids())
            }
        }
    }
    /// Presses a map cell: its run opens with its VM selected, and its serial
    /// is fetched so the pane has something to show at once.
    fn select(&mut self, run: String, vm: u32) -> Effect {
        self.selection = Some(Selection {
            run: run.clone(),
            vm,
        });
        self.show(Pane::Run(run))
    }

    /// Keeps the export box's default pointing at the console's target. The
    /// box keeps a path the operator typed; this only fills in a default.
    fn derive_export(&mut self) {
        if let Pane::Run(id) = &self.pane {
            let id = id.clone();
            let vm = self.target_vm(&id);
            self.export.derive(&id, vm, &export::default_dir());
        }
    }

    /// The transcript write the export button asks for, or why it cannot be
    /// built: no path, no run read yet, or no such VM in the run.
    fn export_call(&self) -> Result<ExportCall, String> {
        let Pane::Run(id) = &self.pane else {
            return Err("export: no run is open".to_string());
        };
        let path = self.export.to_write().to_string();
        if path.is_empty() {
            return Err("export: the path box is empty".to_string());
        }
        let run = self
            .detail
            .as_ref()
            .filter(|run| run.id == *id)
            .ok_or_else(|| format!("export {id}: the run has not been read yet"))?;
        if run.vms.is_empty() {
            return Err(format!("export {id}: the run has no VMs"));
        }
        let (vm, contents) = match self.export.target {
            ExportTarget::Vm => {
                let target = self.target_vm(id);
                let vm = run
                    .vms
                    .iter()
                    .find(|vm| vm.id == target)
                    .ok_or_else(|| format!("export {id}: the run has no vm {target}"))?;
                (Some(vm.id), vm_transcript(run, vm, self.now_ms))
            }
            ExportTarget::Run => (None, run_transcript(run, self.now_ms)),
            ExportTarget::Report => (None, run_report(run, self.now_ms)),
        };
        Ok(ExportCall {
            target: self.export.target,
            run: id.clone(),
            vm,
            path,
            contents,
        })
    }

    /// Presses export: writes the transcript, or says why it cannot.
    fn export_now(&mut self) -> Effect {
        match self.export_call() {
            Ok(call) => Effect {
                export: Some(call),
                ..Effect::none()
            },
            Err(why) => {
                self.notice = Some(Notice {
                    kind: NoticeKind::Failed,
                    text: why,
                });
                Effect::none()
            }
        }
    }

    /// Fills the session CSV's box with its default path, stamped where the
    /// session's first observation landed. A path the operator typed is kept.
    fn derive_csv(&mut self) {
        let at_ms = self
            .history
            .samples()
            .first()
            .map(|sample| sample.at_ms)
            .unwrap_or(self.now_ms);
        self.csv.derive(&export::default_dir(), at_ms);
    }

    /// The session CSV write the export button asks for, or why it cannot be
    /// built: nothing observed yet, or no path to write it to.
    fn csv_call(&self) -> Result<CsvCall, String> {
        if self.history.is_empty() {
            return Err("export: no poll has observed the fleet yet".to_string());
        }
        let path = self.csv.to_write().to_string();
        if path.is_empty() {
            return Err("export: the csv path box is empty".to_string());
        }
        Ok(CsvCall {
            path,
            contents: history::telemetry_csv(&self.history),
        })
    }

    /// Presses the CSV export: writes the session, or says why it cannot.
    fn csv_now(&mut self) -> Effect {
        match self.csv_call() {
            Ok(call) => Effect {
                csv: Some(call),
                ..Effect::none()
            },
            Err(why) => {
                self.notice = Some(Notice {
                    kind: NoticeKind::Failed,
                    text: why,
                });
                Effect::none()
            }
        }
    }

    /// Acts on a key the register binds.
    fn key(&mut self, action: KeyAction) -> Effect {
        // The help sheet is over the register, so while it is up no key
        // reaches a pane: Escape and `?` take it down, everything else waits.
        if self.help {
            if matches!(action, KeyAction::Help | KeyAction::Back) {
                self.help = false;
            }
            return Effect::none();
        }
        match action {
            KeyAction::Register => self.show(Pane::Fleet),
            KeyAction::Machine => self.show(Pane::Machine),
            KeyAction::Map => self.show(Pane::Map),
            KeyAction::Roster => self.show(Pane::Roster),
            KeyAction::Ledger => self.show(Pane::Ledger),
            KeyAction::Treasury => self.show(Pane::Treasury),
            KeyAction::Market => self.show(Pane::Market),
            KeyAction::Telemetry => self.show(Pane::Telemetry),
            KeyAction::Help => {
                self.help = true;
                Effect::none()
            }
            // There is nothing behind the register, so Escape at the register
            // is not an action rather than a reload.
            KeyAction::Back if self.pane == Pane::Fleet => Effect::none(),
            KeyAction::Back => self.show(Pane::Fleet),
            KeyAction::HistoryPrev | KeyAction::HistoryNext => {
                let step = if action == KeyAction::HistoryPrev {
                    HistoryStep::Prev
                } else {
                    HistoryStep::Next
                };
                self.console_history(step)
            }
        }
    }

    /// Walks the console's history: only a run pane has the console on it.
    fn console_history(&mut self, step: HistoryStep) -> Effect {
        if !matches!(self.pane, Pane::Run(_)) {
            return Effect::none();
        }
        self.console.line = self.console.walk(step);
        Effect::none()
    }

    /// Writes `line` to the selected VM's console, when there is a line and a
    /// run to write it to.
    fn console_send(&mut self, line: String) -> Effect {
        let Pane::Run(id) = self.pane.clone() else {
            return Effect::none();
        };
        let line = line.trim();
        if line.is_empty() {
            return Effect::none();
        }
        self.console.remember(line);
        self.console.line.clear();
        self.console.walk = None;
        let vm = self.target_vm(&id);
        Effect::console(ConsoleCall {
            run: id,
            vm,
            data: format!("{line}\n"),
        })
    }

    /// A console write's answer: the run comes back with its serial moved on.
    fn console_answer(
        &mut self,
        at_ms: u64,
        id: &str,
        vm: u32,
        result: Result<Run, ApiError>,
    ) -> Effect {
        match result {
            Ok(run) => {
                self.succeeded(at_ms);
                self.observe(&run);
                if self.pane == Pane::Run(id.to_string()) {
                    self.detail = Some(run);
                }
                Effect::none()
            }
            Err(error) => {
                self.failed(at_ms, &error);
                self.notice = Some(Notice {
                    kind: NoticeKind::Failed,
                    text: format!("console {id} vm {vm}: {error}"),
                });
                Effect::none()
            }
        }
    }

    /// Feeds every VM's serial length into the activity tracker, so the map's
    /// pulse is the delta between this poll and the last, and records the
    /// smoothed rate in the VM's own ring for the run pane's sparkline.
    ///
    /// The length a capture is measured by is the feed's own count, synced
    /// from this very capture: a poll and a stream of deltas then measure the
    /// same capture the same way, and neither looks like growth to the other.
    fn observe(&mut self, run: &Run) {
        for vm in &run.vms {
            let Some(serial) = vm.serial.as_deref() else {
                continue;
            };
            self.serial.sync(&run.id, vm.id, serial);
            let Some(lines) = self.serial.lines(&run.id, vm.id) else {
                continue;
            };
            let lps = self.activity.observe(
                &run.id,
                vm.id,
                usize::try_from(lines).unwrap_or(usize::MAX),
                self.now_ms,
            );
            self.history.observe_vm(&run.id, vm.id, lps);
        }
    }

    /// Submits the spawn form: valid means send, invalid means say what is
    /// wrong and send nothing.
    fn spawn_clicked(&mut self) -> Effect {
        match self.form.validate() {
            Ok(create) => {
                self.errors.clear();
                Effect {
                    spawn: Some(create),
                    ..Effect::none()
                }
            }
            Err(errors) => {
                self.notice = Some(Notice {
                    kind: NoticeKind::Failed,
                    text: format!("the form has {} problem(s)", errors.len()),
                });
                self.errors = errors;
                Effect::none()
            }
        }
    }

    /// Records a call that answered.
    fn succeeded(&mut self, at_ms: u64) {
        self.last_ok_ms = Some(at_ms);
        self.last_error = None;
        self.failures = 0;
        self.next_poll_at_ms = at_ms.saturating_add(self.refresh_ms);
    }

    /// Records a call that failed, and backs the poll off.
    fn failed(&mut self, at_ms: u64, error: &ApiError) {
        self.failures = self.failures.saturating_add(1);
        self.last_error = Some(error.to_string());
        self.next_poll_at_ms = at_ms
            .saturating_add(poll_interval_ms(self.refresh_ms, self.failures));
    }

    /// How fresh the daemon's answers are.
    pub fn health(&self) -> Health {
        if self.failures > 0 && self.last_error.is_some() {
            return Health::Down;
        }
        match self.last_ok_ms {
            None => Health::Unknown,
            Some(last) if self.now_ms.saturating_sub(last) <= STALE_AFTER_MS => Health::Live,
            Some(_) => Health::Stale,
        }
    }

    /// How long ago the last call answered, in milliseconds.
    pub fn age_ms(&self) -> Option<u64> {
        self.last_ok_ms
            .map(|last| self.now_ms.saturating_sub(last))
    }

    /// The spawn form's error for one field, if it has one.
    pub fn error_for(&self, field: Field) -> Option<&str> {
        self.errors
            .iter()
            .find(|(errored, _)| *errored == field)
            .map(|(_, message)| message.as_str())
    }

    /// How many runs the register holds, and how many VMs are up in them.
    pub fn fleet_tally(&self) -> (usize, usize) {
        (
            self.runs.len(),
            self.runs.iter().map(Run::live_vms).sum(),
        )
    }
}

/// How long to wait before the next poll after `failures` have failed in a
/// row: the healthy `refresh_ms`, doubling to a 30 s ceiling.
pub fn poll_interval_ms(refresh_ms: u64, failures: u32) -> u64 {
    let shift = failures.min(4);
    (refresh_ms << shift).min(MAX_POLL_INTERVAL_MS)
}

/// Whether a run has ended, and so will never be live again.
pub fn run_ended(state: RunState) -> bool {
    matches!(state, RunState::Done | RunState::Failed | RunState::Killed)
}

/// Whether a VM has stopped, and so will never be up again.
pub fn vm_stopped(state: VmState) -> bool {
    matches!(state, VmState::Exited | VmState::Killed)
}

/// The state a run moves to, refusing to walk an ended one back.
///
/// The daemon's runs only ever move live to ended, so a view that says
/// otherwise — a list answer read before a transition the register has
/// already seen, or an event that was written before the answer it arrived
/// after — is an older view of the same run, and the register keeps what it
/// saw. A state this console does not know is not treated as ended: it is a
/// newer daemon's word, not a settled one.
pub fn moved_run_state(known: RunState, next: RunState) -> RunState {
    if run_ended(known) && !run_ended(next) {
        known
    } else {
        next
    }
}

/// The same rule for a VM.
pub fn moved_vm_state(known: VmState, next: VmState) -> VmState {
    if vm_stopped(known) && !vm_stopped(next) {
        known
    } else {
        next
    }
}

/// Whether a task has ended, and so will never move again.
pub fn task_ended(state: TaskState) -> bool {
    state.is_finished()
}

/// The state a task moves to, refusing to walk a settled one back.
///
/// The v0 ledger only ever moves a task forward — `open` → `claimed`, then
/// `done`, `failed` or `withdrawn` — so a view that says otherwise describes an
/// older ledger and the console keeps what it saw. A state this console does
/// not know is a newer daemon's word rather than a settled one.
pub fn moved_task_state(known: TaskState, next: TaskState) -> TaskState {
    if task_ended(known) && !task_ended(next) {
        known
    } else {
        next
    }
}

/// The register's half of a roster row: the VM row whose guest runs a role.
#[derive(Debug, Clone, PartialEq)]
pub struct Registered {
    /// The id of the run whose VM row carries the role.
    pub run: String,
    /// The VM's index within that run.
    pub vm: u32,
    /// The role the VM's guest runs as (the daemon's word, as it reported it).
    pub role: String,
    /// The VM's guest CID, when its run has vsock devices: the key the hub's
    /// connected sessions are joined on.
    pub guest_cid: Option<u64>,
    /// The VM's state.
    pub state: VmState,
    /// The VM's serial rate in lines per second, as the activity feed last
    /// measured it (0 until a capture of that VM has been read twice).
    pub lps: f32,
}

impl Registered {
    /// Whether this VM is up right now.
    pub fn is_live(&self) -> bool {
        self.state == VmState::Running
    }
}

/// One agent on the roster: the register's side, the hub's side, or both.
///
/// The two are joined by guest CID and never merged: a row is drawn from the
/// halves the two surfaces actually report, so an agent one of them knows and
/// the other does not is visible as exactly that rather than as a name in the
/// wrong column.
#[derive(Debug, Clone, PartialEq)]
pub struct RosterAgent {
    /// What the register reports: the VM row carrying the role. `None` is an
    /// agent the hub reports connected that no VM row claims.
    pub registered: Option<Registered>,
    /// What the hub reports: the connected keepalive session. `None` is a role
    /// agent the hub reports nobody connected on — when the hub was read at
    /// all, which the row cannot know on its own.
    pub connected: Option<HubAgent>,
}

impl RosterAgent {
    /// The role this row is filed under: the register's word for the VM when
    /// there is one, and the role the hub accepted otherwise.
    pub fn role(&self) -> &str {
        match (&self.registered, &self.connected) {
            (Some(registered), _) => &registered.role,
            (None, Some(connected)) => &connected.role,
            (None, None) => "",
        }
    }

    /// The guest CID the two halves were joined on: the register's VM CID,
    /// falling back to the hub's accepted CID for a row the register does not
    /// report.
    pub fn cid(&self) -> Option<u64> {
        self.registered
            .as_ref()
            .and_then(|registered| registered.guest_cid)
            .or_else(|| self.connected.as_ref().map(|connected| u64::from(connected.cid)))
    }

    /// Whether the register reports this agent's VM.
    pub fn is_registered(&self) -> bool {
        self.registered.is_some()
    }

    /// Whether the hub reports this agent connected.
    pub fn is_connected(&self) -> bool {
        self.connected.is_some()
    }

    /// Whether the register's VM is up right now.
    pub fn is_live(&self) -> bool {
        self.registered.as_ref().is_some_and(Registered::is_live)
    }

    /// The register's serial rate for this agent, 0 without a register half.
    pub fn lps(&self) -> f32 {
        self.registered.as_ref().map_or(0.0, |registered| registered.lps)
    }

    /// The words this row prints for the half it does not have, when it has
    /// only one: the register's row with nobody connected on its CID, or a
    /// connected session no VM row claims. `None` when both halves are there,
    /// and when neither is (which no row of a roster is).
    ///
    /// This is a property of the halves the row holds, and says nothing about
    /// whether the hub was read at all: a row built with no hub has one half
    /// because there is no hub to ask. It is the sheet's business — see
    /// `view::hub_header` — whether that half is *missing* or merely *unasked*,
    /// and it draws these words only when a hub has answered.
    pub fn missing_half(&self) -> Option<&'static str> {
        match (self.registered.is_some(), self.connected.is_some()) {
            (true, false) => Some("registered, not connected"),
            (false, true) => Some("connected, not registered"),
            _ => None,
        }
    }

    /// Whether the hub accepted a different role than the register stamps on
    /// the same CID. The socket is the same one, so a disagreement is a real
    /// observation about the deployment, not a formatting detail.
    pub fn role_mismatch(&self) -> bool {
        match (&self.registered, &self.connected) {
            (Some(registered), Some(connected)) => registered.role != connected.role,
            _ => false,
        }
    }

    /// How long ago this agent last beat, when the hub reports it connected.
    pub fn heartbeat_age_ms(&self, now_ms: u64) -> Option<u64> {
        self.connected
            .as_ref()
            .map(|connected| connected.heartbeat_age_ms(now_ms))
    }
}

/// One role and the agents running it, as the roster groups them.
#[derive(Debug, Clone, PartialEq)]
pub struct RosterGroup {
    /// The role.
    pub role: String,
    /// Every agent carrying it: the register's rows in run-then-VM order,
    /// then the hub's rows the register did not claim, in CID order.
    pub agents: Vec<RosterAgent>,
}

impl RosterGroup {
    /// How many of the role's agents are up.
    pub fn live(&self) -> usize {
        self.agents.iter().filter(|agent| agent.is_live()).count()
    }

    /// How many of the role's agents the hub reports connected.
    pub fn connected(&self) -> usize {
        self.agents.iter().filter(|agent| agent.is_connected()).count()
    }

    /// The role's agents' serial throughput now, in lines per second.
    pub fn lps(&self) -> f32 {
        history::total(self.agents.iter().map(RosterAgent::lps))
    }
}

/// The role fleet as the roster lists it: every VM in the register whose row
/// carries a role, joined by guest CID to the hub's connected sessions, and
/// grouped by role in the order the roles first appear.
///
/// The register's half is a derivation of the live register and nothing else —
/// the daemon is asked for nothing new — so a role appears here exactly when
/// the daemon reports one on a VM row. The hub's half is `hub`'s answer when
/// there is one: an agent it reports connected whose CID no VM row claims is
/// drawn as it is, never folded into a row it does not match. With no hub (the
/// console's default) every row is the register's alone, and the roster is
/// exactly what it was before there was a hub.
pub fn roster(runs: &[Run], activity: &Activity, hub: Option<&HubStatus>) -> Vec<RosterGroup> {
    let mut groups: Vec<RosterGroup> = Vec::new();
    // The hub's rows a register VM claimed, so the leftovers can be drawn as
    // connected-only rather than dropped.
    let mut claimed: Vec<u32> = Vec::new();

    for run in runs {
        for vm in &run.vms {
            let Some(role) = vm.role.as_deref() else {
                continue;
            };
            let connected = hub
                .and_then(|status| vm.guest_cid.and_then(|cid| status.agent(cid)))
                .cloned();
            if let Some(agent) = &connected {
                claimed.push(agent.cid);
            }
            file(&mut groups, RosterAgent {
                registered: Some(Registered {
                    run: run.id.clone(),
                    vm: vm.id,
                    role: role.to_string(),
                    guest_cid: vm.guest_cid,
                    state: vm.state,
                    lps: activity.lps(&run.id, vm.id),
                }),
                connected,
            });
        }
    }

    // The hub's other half: an agent it accepted that no VM row claims. It is
    // drawn where it is — connected, not registered — so a session the
    // register does not account for is never silently dropped.
    if let Some(status) = hub {
        for connected in &status.agents {
            if claimed.contains(&connected.cid) {
                continue;
            }
            file(&mut groups, RosterAgent {
                registered: None,
                connected: Some(connected.clone()),
            });
        }
    }

    groups
}

/// Files `agent` under its role, in the order the roles first appear.
fn file(groups: &mut Vec<RosterGroup>, agent: RosterAgent) {
    let role = agent.role().to_string();
    match groups.iter_mut().find(|group| group.role == role) {
        Some(group) => group.agents.push(agent),
        None => groups.push(RosterGroup {
            role,
            agents: vec![agent],
        }),
    }
}

/// The roster console's args box, parsed as the JSON object the hub's `/tell`
/// expects.
///
/// An empty box is `{}` — which is what a tool taking no arguments (`uptime`)
/// wants — and anything that is not a JSON object is refused with the hub's own
/// wording, so the same complaint reads whether this console or the hub said
/// it.
pub fn parse_args(text: &str) -> Result<serde_json::Value, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(serde_json::Value::Object(serde_json::Map::new()));
    }
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value @ serde_json::Value::Object(_)) => Ok(value),
        _ => Err("args must be a JSON object".to_string()),
    }
}

/// The drive state an `ApiError` names: the hub's own refusal (its `403` role
/// policy, or another status), or the admin surface being unreadable.
fn drive_error(cid: u64, name: String, error: ApiError) -> DriveState {
    match error {
        ApiError::Status { status, message } => DriveState::Refused {
            cid,
            name,
            status,
            message,
        },
        error => DriveState::Unreachable {
            cid,
            name,
            message: error.to_string(),
        },
    }
}

/// The stop state an `ApiError` names: the hub's own refusal (`404` a CID it
/// does not serve, `409` a served CID with no live session), or the admin
/// surface being unreadable.
fn stop_error(cid: u64, error: ApiError) -> StopState {
    match error {
        ApiError::Status { status, message } => StopState::Refused {
            cid,
            status,
            message,
        },
        error => StopState::Unreachable {
            cid,
            message: error.to_string(),
        },
    }
}

/// The hub's header line, as the roster writes it: what it serves, who is
/// connected, the live sessions, and the daemon's uptime.
pub fn hub_line(status: &HubStatus) -> String {
    format!(
        "hub · {} cid(s) served · {} agent(s) connected · {} session(s) active · up {}",
        status.node.cids.len(),
        status.agents.len(),
        status.sessions_active,
        duration_label(status.uptime_ms())
    )
}

/// The hub's capacity line: the facts the frozen body carries and nothing else
/// on the sheet says — which CIDs it serves, on which vsock port, how full its
/// session budget is, and how long a session may go quiet before it is
/// dropped.
pub fn hub_facts(status: &HubStatus) -> String {
    format!(
        "serves {} on vsock port {} · {}/{} session(s) · keepalive idle {}s",
        cid_span(&status.node.cids),
        status.node.port,
        status.sessions_active,
        status.node.max_sessions,
        status.node.keepalive_idle_s
    )
}

/// How the CIDs a hub serves are written: the single CID, the first-to-last
/// span, or the honest nothing.
fn cid_span(cids: &[u32]) -> String {
    match (cids.first(), cids.last()) {
        (Some(first), Some(last)) if first == last => format!("cid {first}"),
        (Some(first), Some(last)) => format!("cid {first}..{last}"),
        _ => "no cid".to_string(),
    }
}

/// The window title: the register's name and the sheet that is showing.
pub fn title(model: &Model) -> String {
    match &model.pane {
        Pane::Fleet => "AgentPlaza register — fleet".to_string(),
        Pane::Run(id) => format!("AgentPlaza register — {id}"),
        Pane::Spawn => "AgentPlaza register — new run".to_string(),
        Pane::Machine => "AgentPlaza register — machine".to_string(),
        Pane::Map => "AgentPlaza register — fleet map".to_string(),
        Pane::Roster => "AgentPlaza register — roster".to_string(),
        Pane::Ledger => "AgentPlaza register — ledger".to_string(),
        Pane::Treasury => "AgentPlaza register — treasury".to_string(),
        Pane::Market => "AgentPlaza register — market".to_string(),
        Pane::Telemetry => "AgentPlaza register — telemetry".to_string(),
    }
}

/// The fleet tally as the register writes it: the runs, the VMs up, the
/// distinct guest CIDs seen and the runs carrying a vsock device.
pub fn tally_line(model: &Model) -> String {
    let (runs, vms) = model.fleet_tally();
    format!(
        "{runs} run(s) · {vms} vm(s) up · {} cid(s) · {} vsock run(s)",
        model.guest_cids().len(),
        model.vsock_runs()
    )
}

/// The roster's tally as the roster sheet writes it: the agents the sheet
/// carries (a register row or a connected session — a row is an agent), how
/// many of their VMs are up right now, the roles they are filed under, and the
/// serial throughput the register's half is writing between polls.
pub fn roster_tally(groups: &[RosterGroup]) -> String {
    let agents: usize = groups.iter().map(|group| group.agents.len()).sum();
    let live: usize = groups.iter().map(RosterGroup::live).sum();
    format!(
        "{agents} agent(s) · {live} up right now · {} role(s) · {:.1} line(s)/s",
        groups.len(),
        history::total(groups.iter().map(RosterGroup::lps))
    )
}

/// The roster as the index column writes it: the roles the register reports
/// and how many of their agents are up, and the honest nothing when no VM
/// carries a role at all (an older daemon, or one not started with
/// `--agents`).
///
/// With a hub that has answered, the hub's own connected count joins the line.
/// The index is what the operator reads first, and an entry that says "21
/// agent(s) up" while the sheet's header says "21 agent(s) connected" leaves
/// them to scroll to find out whether those are the same twenty-one.
pub fn roster_line(model: &Model) -> String {
    let groups = model.roster();
    if groups.is_empty() {
        return "no role agent yet".to_string();
    }
    let up = groups.iter().map(RosterGroup::live).sum::<usize>();
    let mut line = format!("{} role(s) · {up} agent(s) up", groups.len());
    if let Some(connected) = model.hub_connected() {
        line.push_str(&format!(" · {connected} connected"));
    }
    line
}

/// The ledger as the index column writes it: the counts once the ledger has
/// answered, and the honest nothing before that.
///
/// A ledger nobody has read is not an empty ledger: the entry the operator
/// reads first says which of the two it is rather than claiming a count nobody
/// took.
pub fn task_line(model: &Model) -> String {
    if model.ledger_read {
        task_tally(&model.tasks)
    } else {
        "not read yet".to_string()
    }
}

/// How the ledger stands, as the sheet's tally and the index entry write it:
/// the tasks the daemon holds, how many wait for an agent, how many are in an
/// agent's hands, and how many have ended.
pub fn task_tally(tasks: &[Task]) -> String {
    let count = |state: TaskState| tasks.iter().filter(|task| task.state == state).count();
    format!(
        "{} task(s) · {} open · {} claimed · {} ended",
        tasks.len(),
        count(TaskState::Open),
        count(TaskState::Claimed),
        tasks.iter().filter(|task| task.state.is_finished()).count()
    )
}

/// A task's state, and — once it has ended — the schema's outcome beside it,
/// as the ledger's detail writes the pair. The two are not the same word: a
/// task ends `done` holding the outcome `accepted`.
pub fn task_state_line(task: &Task) -> String {
    match task.outcome.get() {
        Some(outcome) => format!("{} · {}", task.state.label(), outcome.label()),
        None => task.state.label().to_string(),
    }
}

/// An amount of credits as the register's money: the ledger's own unit and the
/// dollars it stands for.
///
/// Amounts are whole credits and 1 credit = $0.001 of real spend
/// (`docs/treasury-draft/schema.sql`, statute 0109), so a figure is always
/// shown both ways and no rate is invented — the same idiom the web console's
/// money helper uses. A negative amount (a debit, or an account below zero) is
/// written with the sign in front of the dollars too.
pub fn money_label(credits: i128) -> String {
    let dollars = credits as f64 / 1_000.0;
    if credits < 0 {
        format!("{credits} cr · -${:.2}", -dollars)
    } else {
        format!("{credits} cr · ${dollars:.2}")
    }
}

/// The treasury as the index column writes it: the counts once the pane has
/// read it, and the honest nothing before that.
///
/// A treasury nobody has read is not an empty treasury: the entry the operator
/// reads first says which of the two it is rather than claiming a count nobody
/// took — the ledger's own rule.
pub fn treasury_line(model: &Model) -> String {
    match &model.treasury {
        Some(read) => format!(
            "{} account(s) · {} entry(s)",
            read.treasury.totals.accounts, read.treasury.totals.entries
        ),
        None => "not read yet".to_string(),
    }
}

/// The treasury's totals as the sheet writes them: the accounts, the entries,
/// what the journal has credited and debited, and the sum of every balance —
/// zero while the journal balances.
pub fn totals_line(totals: &Totals) -> String {
    format!(
        "{} account(s) · {} entry(s) · credits {} · debits {} · balance {}",
        totals.accounts,
        totals.entries,
        money_label(totals.credits),
        money_label(totals.debits),
        money_label(totals.balance),
    )
}

/// One posting as the journal's ruled pair writes it: the account, by its kind
/// and its owner.
pub fn posting_account(posting: &Posting) -> String {
    format!("{} · {}", posting.kind.label(), posting.owner_ref)
}

/// An entry's task as its row writes it: the task the entry belongs to, or the
/// honest dash for an entry that belongs to none (a `fine` or a `fee`).
pub fn entry_task(entry: &Entry) -> &str {
    entry.task_id.as_deref().unwrap_or("—")
}

/// How the journal stands, as the sheet's tally writes it: the entries the
/// response carries, the journal's own total, and whether the read was cut to
/// the daemon's limit.
pub fn journal_tally(journal: &Journal) -> String {
    let mut line = format!(
        "{} entry(s) carried of {} in the journal",
        journal.entries.len(),
        journal.total
    );
    if journal.truncated {
        line.push_str(&format!(" · cut to the newest {}", journal.limit));
    }
    line
}

/// The market as the index column writes it: the calls and the bids once the
/// pane has read the board, and the honest nothing before that.
///
/// A board nobody has read is not an empty board: the entry the operator reads
/// first says which of the two it is rather than claiming a count nobody took
/// — the ledger's and the treasury's own rule.
pub fn market_line(model: &Model) -> String {
    match &model.agora {
        Some(board) => format!("{} call(s) · {} bid(s)", board.total, bid_count(board)),
        None => "not read yet".to_string(),
    }
}

/// How many bids the board's calls hold.
fn bid_count(board: &Board) -> usize {
    board.calls.iter().map(|call| call.bids.len()).sum()
}

/// How the market stands, as the sheet's tally writes it: the calls the
/// response carries, the board's own total, how many bids they hold, how many
/// closed with an award, and whether the read was cut to the daemon's limit.
pub fn board_tally(board: &Board) -> String {
    let awarded = board.calls.iter().filter(|call| call.award.get().is_some()).count();
    let mut line = format!(
        "{} call(s) carried of {} · {} bid(s) · {awarded} awarded",
        board.calls.len(),
        board.total,
        bid_count(board),
    );
    if board.truncated {
        line.push_str(&format!(" · cut to the newest {}", board.limit));
    }
    line
}

/// An award as its row writes it: who took the call, at what price, and the
/// treasury entry that funded it — labelled as the treasury's own funding
/// entry, because the market does not jump to that sheet to draw it.
pub fn award_line(award: &Award) -> String {
    let price = money_label(i128::from(award.price));
    match award.escrow_entry.copied() {
        Some(entry) => format!(
            "awarded to {} · cid {} · {} · escrow entry {entry} (the treasury's funding entry)",
            award.contractor, award.agent_cid, price
        ),
        None => format!(
            "awarded to {} · cid {} · {} · escrow entry none (a call whose task named no budget)",
            award.contractor, award.agent_cid, price
        ),
    }
}

/// Guest memory as the register writes it: MiB below a gibibyte (where a
/// fraction would say less), gibibytes to one decimal above it.
pub fn memory_label(mib: u64) -> String {
    if mib < 1_024 {
        return format!("{mib} MiB");
    }
    // Rounded half up: a footprint hint that understates is a hint that lies.
    format!("{:.1} GiB", (mib as f64 * 10.0 / 1_024.0).round() / 10.0)
}

/// A preset's footprint as the static label text beside its button: the VMs it
/// boots, the memory each is given, and the total that asks the host for.
pub fn preset_footprint(preset: Preset) -> String {
    let shape = preset.shape();
    format!(
        "{} × {} MiB ≈ {} guest RAM",
        shape.n,
        shape.memory_mib,
        memory_label(shape.total_mib())
    )
}

/// One line of a serial capture, as the transcript draws it: where the line
/// starts in the capture, and what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptLine<'a> {
    /// The byte of the capture the line begins at, counted from its start.
    pub offset: u64,
    /// The line, without its terminator.
    pub text: &'a str,
}

/// The last `limit` lines of a capture, and how many earlier ones were left
/// out above them: each line with the byte it starts at.
///
/// The offset is the register's own ledger habit — the draft's transcript puts
/// every row's sequence number in the margin — over the measure this console
/// actually has: `serial` events carry the byte offset their text starts at,
/// and this is the same number counted over a capture read whole. The margin's
/// figures only ever go up, which is exactly what the offset-idempotent merge
/// promises.
pub fn serial_tail(capture: &str, limit: usize) -> (usize, Vec<TranscriptLine<'_>>) {
    let mut rows: Vec<TranscriptLine<'_>> = Vec::new();
    let mut offset = 0u64;
    if !capture.is_empty() {
        for line in capture.split_inclusive('\n') {
            let text = match line.strip_suffix('\n') {
                Some(rest) => rest.strip_suffix('\r').unwrap_or(rest),
                None => line,
            };
            rows.push(TranscriptLine { offset, text });
            offset += line.len() as u64;
        }
    }
    let skipped = rows.len().saturating_sub(limit);
    rows.drain(..skipped);
    (skipped, rows)
}

/// The span of a capture the transcript is drawing: from the first drawn
/// line's byte to the capture's end, as the heading over the margin numbers
/// names it. An empty capture has no span to name.
pub fn transcript_span<'a>(
    capture: &str,
    rows: &[TranscriptLine<'a>],
) -> Option<(u64, u64)> {
    let first = rows.first()?;
    Some((first.offset, capture.len() as u64))
}

/// A duration as the register writes it: seconds, then minutes, then hours,
/// then days.
///
/// A node's uptime and a fleet run's age are hours-to-days figures — a run
/// that has been `running` since the day before yesterday must read as days,
/// not as an unbounded hour count — so the ladder tops out at days rather than
/// running on in hours forever.
pub fn duration_label(ms: u64) -> String {
    let seconds = ms / 1_000;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m {:02}s", seconds % 60);
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h {:02}m", minutes % 60);
    }
    format!("{}d {:02}h", hours / 24, hours % 24)
}

/// One day, in milliseconds: the register's own day, for the date it prints
/// beside a time. A UTC day, because every clock this register writes is UTC.
pub const DAY_MS: u64 = 86_400_000;

/// A wall-clock time as the register writes it, in UTC.
pub fn clock_label(epoch_ms: u64) -> String {
    let seconds = epoch_ms / 1_000;
    format!(
        "{:02}:{:02}:{:02} UTC",
        (seconds / 3_600) % 24,
        (seconds / 60) % 60,
        seconds % 60
    )
}

/// A UTC date as the register writes it: `2026-10-07`.
pub fn date_label(epoch_ms: u64) -> String {
    let days = epoch_ms / DAY_MS;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// An instant as the register writes it, read against `reference_ms`: the
/// clock when the instant and the reference are on the same UTC day, and the
/// date when they are not.
///
/// The register always prints an instant *beside* its age (`created
/// 14:03:22 UTC · 3d 02h`), and a fleet run stays `running` for days at a
/// time. A bare time of day read three days later names no particular instant
/// — it is today's clock applied to Wednesday — so on another day the day is
/// what the register writes, and the age beside it says the rest.
pub fn instant_label(epoch_ms: u64, reference_ms: u64) -> String {
    if epoch_ms / DAY_MS == reference_ms / DAY_MS {
        clock_label(epoch_ms)
    } else {
        date_label(epoch_ms)
    }
}

/// The civil date of a count of days since 1970-01-01, as `(year, month,
/// day)`: Howard Hinnant's `civil_from_days`, which is exact for every day the
/// register can hold.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{EntryKind, Vm};

    fn vm(id: u32, state: VmState, cid: Option<u64>, serial: Option<&str>) -> Vm {
        Vm {
            id,
            guest_cid: cid,
            role: None,
            state,
            serial: serial.map(str::to_string),
        }
    }

    fn run(id: &str, state: RunState) -> Run {
        Run {
            id: id.to_string(),
            state,
            vcpus: 2,
            memory_mib: 512,
            beats: None,
            created_at: 1_000_000,
            ended_at: None,
            vms: vec![vm(0, VmState::Running, Some(50), None)],
        }
    }

    fn page(runs: Vec<Run>) -> Result<RunsPage, ApiError> {
        Ok(RunsPage { runs })
    }

    #[test]
    fn the_first_tick_polls_immediately_and_repeats_at_two_seconds() {
        let mut model = Model::new();
        assert_eq!(model.apply(Message::Tick { at_ms: 1_000 }), Effect::poll());
        assert_eq!(model.polls, 1);

        // A tick before the schedule says nothing.
        assert!(model.apply(Message::Tick { at_ms: 1_500 }).is_idle());
        assert_eq!(model.polls, 1);

        model.apply(Message::Runs { at_ms: 1_100, result: page(vec![run("r1", RunState::Running)]) });
        assert_eq!(model.next_poll_at_ms, 3_100);
        assert!(model.apply(Message::Tick { at_ms: 3_000 }).is_idle());
        assert_eq!(model.apply(Message::Tick { at_ms: 3_100 }), Effect::poll());
        assert_eq!(model.polls, 2);
    }

    #[test]
    fn a_failing_poll_backs_off_and_a_success_returns_to_two_seconds() {
        let base = DEFAULT_REFRESH_MS;
        assert_eq!(poll_interval_ms(base, 0), 2_000);
        assert_eq!(poll_interval_ms(base, 1), 4_000);
        assert_eq!(poll_interval_ms(base, 2), 8_000);
        assert_eq!(poll_interval_ms(base, 3), 16_000);
        assert_eq!(poll_interval_ms(base, 4), 30_000, "capped");
        assert_eq!(poll_interval_ms(base, 40), 30_000);

        let mut model = Model::new();
        let error = ApiError::Connect("127.0.0.1:8899: refused".to_string());
        model.apply(Message::Runs {
            at_ms: 1_000,
            result: Err(error.clone()),
        });
        assert_eq!(model.next_poll_at_ms, 1_000 + 4_000);
        assert_eq!(model.health(), Health::Down);
        assert_eq!(model.last_error, Some(error.to_string()));

        model.apply(Message::Runs {
            at_ms: 2_000,
            result: Err(error),
        });
        assert_eq!(model.next_poll_at_ms, 2_000 + 8_000);

        model.apply(Message::Runs {
            at_ms: 9_000,
            result: page(vec![]),
        });
        assert_eq!(model.failures, 0);
        assert_eq!(model.next_poll_at_ms, 9_000 + DEFAULT_REFRESH_MS);
        assert_eq!(model.health(), Health::Live);
    }

    #[test]
    fn a_cadence_other_than_two_seconds_sets_the_poll_and_the_backoff() {
        // A register asked for a slower cadence backs off from there, and is
        // never sped up by the ceiling.
        let mut model = Model::at_cadence(Endpoint::default(), 5_000);
        assert_eq!(model.refresh_ms, 5_000);
        assert_eq!(model.apply(Message::Tick { at_ms: 1_000 }), Effect::poll());
        assert_eq!(model.next_poll_at_ms, 6_000);
        model.apply(Message::Runs {
            at_ms: 1_100,
            result: page(vec![]),
        });
        assert_eq!(model.next_poll_at_ms, 6_100, "5 s after the answer");

        model.apply(Message::Runs {
            at_ms: 7_000,
            result: Err(ApiError::Connect("refused".to_string())),
        });
        assert_eq!(model.next_poll_at_ms, 7_000 + 10_000);

        // At the ceiling the cadence is the ceiling: the backoff cannot slow
        // it below what was asked for, and cannot speed it up either.
        let mut slow = Model::at_cadence(Endpoint::default(), MAX_REFRESH_MS);
        slow.apply(Message::Runs {
            at_ms: 1_000,
            result: Err(ApiError::Connect("refused".to_string())),
        });
        assert_eq!(slow.next_poll_at_ms, 1_000 + MAX_REFRESH_MS);
    }

    #[test]
    fn health_reads_unknown_then_live_then_stale() {
        let mut model = Model::new();
        assert_eq!(model.health(), Health::Unknown);
        assert_eq!(model.age_ms(), None);

        model.apply(Message::Runs { at_ms: 10_000, result: page(vec![]) });
        assert_eq!(model.health(), Health::Live);
        assert_eq!(model.age_ms(), Some(0));

        model.apply(Message::Tick { at_ms: 15_000 });
        assert_eq!(model.health(), Health::Live, "exactly at the stale bound");

        model.apply(Message::Tick { at_ms: 15_001 });
        assert_eq!(model.health(), Health::Stale);
        assert_eq!(model.age_ms(), Some(5_001));
    }

    #[test]
    fn a_404_on_a_run_keeps_the_daemon_live_and_closes_the_pane() {
        let mut model = Model::new();
        model.apply(Message::Show(Pane::Run("r9".to_string())));
        assert_eq!(model.pane, Pane::Run("r9".to_string()));
        assert_eq!(model.armed, None);

        let gone = ApiError::Status {
            status: 404,
            message: "no run with id r9".to_string(),
        };
        let effect = model.apply(Message::Run {
            at_ms: 5_000,
            id: "r9".to_string(),
            result: Err(gone),
        });

        assert!(effect.is_idle());
        assert_eq!(model.pane, Pane::Fleet);
        assert_eq!(model.detail, None);
        assert_eq!(model.health(), Health::Live, "a 404 proves the daemon is up");
        assert_eq!(
            model.notice.as_ref().expect("a notice").text,
            "run r9 is gone"
        );
    }

    #[test]
    fn showing_a_run_fetches_it_and_follows() {
        let mut model = Model::new();
        model.follow = false;
        let effect = model.apply(Message::Show(Pane::Run("r1".to_string())));
        assert_eq!(effect, Effect::fetch("r1"));
        assert!(model.follow, "opening a run follows it");

        model.apply(Message::Run {
            at_ms: 1_000,
            id: "r1".to_string(),
            result: Ok(run("r1", RunState::Running)),
        });
        assert_eq!(model.detail.as_ref().expect("the run").id, "r1");

        // Following polls the list and the run together.
        let effect = model.apply(Message::Tick { at_ms: 3_000 });
        assert_eq!(effect, Effect::refresh("r1"));

        // Not following polls the list only.
        model.apply(Message::Follow(false));
        let effect = model.apply(Message::Tick { at_ms: 5_000 });
        assert_eq!(effect, Effect::poll());
    }

    #[test]
    fn a_tick_while_not_on_a_run_does_not_fetch_one() {
        let mut model = Model::new();
        model.apply(Message::Show(Pane::Fleet));
        model.next_poll_at_ms = 0;
        assert_eq!(model.apply(Message::Tick { at_ms: 10 }), Effect::poll());
    }

    #[test]
    fn a_repeated_show_of_the_same_run_keeps_the_loaded_serial() {
        let mut model = Model::new();
        model.apply(Message::Show(Pane::Run("r1".to_string())));
        let mut detail = run("r1", RunState::Running);
        detail.vms[0].serial = Some("$ uname\nLinux\n".to_string());
        model.detail = Some(detail);

        model.apply(Message::Show(Pane::Run("r1".to_string())));
        assert_eq!(
            model.detail.as_ref().expect("still loaded").serial(),
            "$ uname\nLinux\n"
        );
    }

    #[test]
    fn a_detail_for_another_run_is_ignored() {
        let mut model = Model::new();
        model.apply(Message::Show(Pane::Run("r1".to_string())));
        model.apply(Message::Run {
            at_ms: 1_000,
            id: "r2".to_string(),
            result: Ok(run("r2", RunState::Running)),
        });
        assert_eq!(model.detail, None);
    }

    #[test]
    fn killing_takes_two_presses_and_re_arms_on_another_run() {
        let mut model = Model::new();
        assert!(model.apply(Message::KillClicked("r1".to_string())).is_idle());
        assert_eq!(model.armed.as_deref(), Some("r1"));

        // Pressing another run's kill moves the arm.
        assert!(model.apply(Message::KillClicked("r2".to_string())).is_idle());
        assert_eq!(model.armed.as_deref(), Some("r2"));

        assert_eq!(
            model.apply(Message::KillClicked("r2".to_string())),
            Effect {
                kill: Some("r2".to_string()),
                ..Effect::none()
            }
        );
        assert_eq!(model.armed, None, "the arm is spent");
    }

    #[test]
    fn a_kill_answer_refreshes_the_register_and_shows_the_final_state() {
        let mut model = Model::new();
        model.apply(Message::Show(Pane::Run("r1".to_string())));
        model.armed = Some("r1".to_string());

        let mut killed = run("r1", RunState::Killed);
        killed.ended_at = Some(1_000_500);
        killed.vms[0].state = VmState::Killed;

        let effect = model.apply(Message::Killed {
            at_ms: 1_000_600,
            id: "r1".to_string(),
            result: Ok(killed),
        });

        assert_eq!(effect, Effect::refresh("r1"));
        assert_eq!(model.armed, None);
        assert_eq!(model.detail.as_ref().expect("kept").state, RunState::Killed);
        assert_eq!(model.notice.as_ref().expect("notice").text, "run r1 killed");
    }

    #[test]
    fn a_failed_kill_is_surfaced_not_swallowed() {
        let mut model = Model::new();
        model.armed = Some("r1".to_string());
        model.apply(Message::Killed {
            at_ms: 1_000,
            id: "r1".to_string(),
            result: Err(ApiError::Status {
                status: 404,
                message: "no run with id r1".to_string(),
            }),
        });
        assert_eq!(model.armed, None);
        let notice = model.notice.as_ref().expect("a notice");
        assert_eq!(notice.kind, NoticeKind::Failed);
        assert_eq!(notice.text, "run r1: 404: no run with id r1");
        assert_eq!(model.health(), Health::Down, "a refused kill is a failure");
    }

    #[test]
    fn a_valid_form_submits_and_an_invalid_one_reports_itself() {
        let mut model = Model::new();
        assert_eq!(
            model.apply(Message::SpawnClicked),
            Effect {
                spawn: Some(CreateRun {
                    n: 1,
                    vcpus: 2,
                    memory_mib: 512,
                    beats: None,
                    vsock_cid: None,
                }),
                ..Effect::none()
            }
        );

        model.apply(Message::Edit {
            field: Field::MemoryMib,
            value: "16".to_string(),
        });
        let effect = model.apply(Message::SpawnClicked);
        assert!(effect.is_idle(), "an invalid form sends nothing");
        assert_eq!(
            model.error_for(Field::MemoryMib),
            Some("memory_mib must be at least 128")
        );
        assert_eq!(
            model.notice.as_ref().expect("notice").kind,
            NoticeKind::Failed
        );

        model.apply(Message::Edit {
            field: Field::MemoryMib,
            value: "128".to_string(),
        });
        assert_eq!(model.error_for(Field::MemoryMib), None);
        assert!(matches!(
            model.apply(Message::SpawnClicked),
            Effect { spawn: Some(_), .. }
        ));
    }

    #[test]
    fn a_created_run_becomes_the_selected_pane_and_is_fetched() {
        let mut model = Model::new();
        model.apply(Message::Show(Pane::Spawn));
        let effect = model.apply(Message::Spawned {
            at_ms: 2_000,
            result: Ok(run("r7", RunState::Running)),
        });

        assert_eq!(effect, Effect::refresh("r7"));
        assert_eq!(model.pane, Pane::Run("r7".to_string()));
        assert_eq!(model.detail.as_ref().expect("the run").id, "r7");
        assert_eq!(model.notice.as_ref().expect("notice").text, "run r7 created");
    }

    #[test]
    fn a_refused_spawn_is_surfaced() {
        let mut model = Model::new();
        model.apply(Message::Spawned {
            at_ms: 2_000,
            result: Err(ApiError::Status {
                status: 400,
                message: "vcpus must be between 1 and 64".to_string(),
            }),
        });
        assert_eq!(
            model.notice.as_ref().expect("notice").text,
            "spawn refused: 400: vcpus must be between 1 and 64"
        );
        assert_eq!(model.pane, Pane::Fleet, "the form keeps its place");
        assert_eq!(model.health(), Health::Down);
    }

    #[test]
    fn showing_the_fleet_clears_the_detail_and_polls() {
        let mut model = Model::new();
        model.apply(Message::Show(Pane::Run("r1".to_string())));
        model.detail = Some(run("r1", RunState::Running));
        model.armed = Some("r1".to_string());

        assert_eq!(model.apply(Message::Show(Pane::Fleet)), Effect::poll());
        assert_eq!(model.pane, Pane::Fleet);
        assert_eq!(model.detail, None);
        assert_eq!(model.armed, None);
    }

    #[test]
    fn showing_the_spawn_form_drops_the_last_submissions_errors() {
        let mut model = Model::new();
        model.errors.push((Field::N, "n must be between 1 and 64".to_string()));
        assert!(model.apply(Message::Show(Pane::Spawn)).is_idle());
        assert!(model.errors.is_empty());
        assert_eq!(model.pane, Pane::Spawn);
    }

    #[test]
    fn the_register_tally_counts_runs_and_live_vms() {
        let mut model = Model::new();
        let mut stopped = run("r2", RunState::Done);
        stopped.vms[0].state = VmState::Killed;
        stopped.vms.push(vm(1, VmState::Killed, Some(51), None));
        model.apply(Message::Runs {
            at_ms: 1_000,
            result: page(vec![run("r1", RunState::Running), stopped]),
        });
        assert_eq!(model.fleet_tally(), (2, 1));
    }

    #[test]
    fn a_notice_can_be_dismissed() {
        let mut model = Model::new();
        model.notice = Some(Notice {
            kind: NoticeKind::Done,
            text: "run r1 killed".to_string(),
        });
        assert!(model.apply(Message::DismissNotice).is_idle());
        assert_eq!(model.notice, None);
    }

    #[test]
    fn the_serial_tail_keeps_the_last_lines_and_counts_the_rest() {
        let capture = "one\ntwo\nthree\nfour\n";
        let (skipped, rows) = serial_tail(capture, 2);
        assert_eq!(skipped, 2);
        assert_eq!(
            rows,
            vec![
                TranscriptLine {
                    offset: 8,
                    text: "three"
                },
                TranscriptLine {
                    offset: 14,
                    text: "four"
                },
            ],
            "each row carries the byte it starts at"
        );

        let (skipped, rows) = serial_tail(capture, SERIAL_TAIL_LINES);
        assert_eq!(skipped, 0);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].offset, 0, "nothing was left out, so the span opens at 0");
        assert_eq!(transcript_span(capture, &rows), Some((0, capture.len() as u64)));

        let (skipped, rows) = serial_tail("", 4);
        assert_eq!((skipped, rows.len()), (0, 0));
        assert_eq!(transcript_span("", &rows), None, "an empty capture has no span");
    }

    #[test]
    fn a_serial_captures_carriage_returns_do_not_reach_the_lines() {
        let capture = "login: plaza\r\n$ uname\r\nLinux\r\n";
        let (_, rows) = serial_tail(capture, 10);
        assert_eq!(
            rows.iter().map(|row| row.text).collect::<Vec<_>>(),
            vec!["login: plaza", "$ uname", "Linux"]
        );
        // The offsets count the capture's own bytes, terminators and all.
        assert_eq!(
            rows.iter().map(|row| row.offset).collect::<Vec<_>>(),
            vec![0, 14, 23]
        );
        assert_eq!(transcript_span(capture, &rows), Some((0, 30)));
    }

    #[test]
    fn a_capture_the_guest_has_not_finished_writing_still_has_a_line() {
        let capture = "one\ntwo";
        let (_, rows) = serial_tail(capture, 10);
        assert_eq!(
            rows,
            vec![
                TranscriptLine {
                    offset: 0,
                    text: "one"
                },
                TranscriptLine {
                    offset: 4,
                    text: "two"
                },
            ],
            "a trailing partial line is drawn where the guest left it"
        );
        assert_eq!(transcript_span(capture, &rows), Some((0, 7)));
    }

    #[test]
    fn durations_read_as_seconds_minutes_and_hours() {
        assert_eq!(duration_label(0), "0s");
        assert_eq!(duration_label(999), "0s");
        assert_eq!(duration_label(59_999), "59s");
        assert_eq!(duration_label(60_000), "1m 00s");
        assert_eq!(duration_label(125_000), "2m 05s");
        assert_eq!(duration_label(3_600_000), "1h 00m");
        assert_eq!(duration_label(7_860_000), "2h 11m");
    }

    #[test]
    fn a_long_running_run_reads_in_days_rather_than_endless_hours() {
        // The hour tier is where a fleet run lives for its first day...
        assert_eq!(duration_label(23 * 3_600_000 + 59 * 60_000 + 59_000), "23h 59m");
        // ...and a run still `running` after that is a day old, not "24h".
        assert_eq!(duration_label(24 * 3_600_000), "1d 00h");
        assert_eq!(duration_label(25 * 3_600_000 + 7 * 60_000), "1d 01h");
        assert_eq!(duration_label(72 * 3_600_000 + 5 * 60_000), "3d 00h");
        assert_eq!(duration_label(100 * 3_600_000 + 30 * 60_000), "4d 04h");
        assert_eq!(duration_label(30 * 24 * 3_600_000), "30d 00h");
        // Never a mislabelled tier: the ladder only ever moves up whole
        // hours into whole days, and a minute is never lost to rounding.
        assert_eq!(duration_label(24 * 3_600_000 - 1), "23h 59m");
    }

    #[test]
    fn clock_labels_are_utc() {
        assert_eq!(clock_label(0), "00:00:00 UTC");
        assert_eq!(clock_label(1_760_000_000_000), "08:53:20 UTC");
        assert_eq!(clock_label(86_400_000 + 3_661_000), "01:01:01 UTC");
    }

    #[test]
    fn dates_are_the_utc_civil_date() {
        assert_eq!(date_label(0), "1970-01-01");
        assert_eq!(date_label(DAY_MS - 1), "1970-01-01");
        assert_eq!(date_label(DAY_MS), "1970-01-02");
        // A leap day, and a year boundary: the day count must not drift.
        assert_eq!(date_label(11_016 * DAY_MS), "2000-02-29");
        assert_eq!(date_label(19_723 * DAY_MS), "2024-01-01");
        assert_eq!(date_label(1_760_000_000_000), "2025-10-09");
    }

    #[test]
    fn an_instant_on_another_day_is_written_as_its_day() {
        let noon = 1_760_000_000_000; // 2025-10-09 08:53:20 UTC
        // The same UTC day keeps the clock the register has always written.
        assert_eq!(instant_label(noon, noon + 3 * 3_600_000), "08:53:20 UTC");
        assert_eq!(
            instant_label(noon, noon + 15 * 3_600_000 + 6 * 60_000),
            "08:53:20 UTC"
        );
        // Another day is a day: a bare time of day about a run that began on
        // Wednesday would read as today's clock.
        assert_eq!(instant_label(noon, noon + DAY_MS), "2025-10-09");
        assert_eq!(instant_label(noon - DAY_MS, noon), "2025-10-08");
        assert_eq!(instant_label(0, 1_760_000_000_000), "1970-01-01");
        // Across the UTC day boundary, one second either side of midnight.
        let midnight = DAY_MS * 20_000; // 2024-10-04 00:00:00 UTC
        assert_eq!(instant_label(midnight - 1_000, midnight), "2024-10-03");
        assert_eq!(instant_label(midnight, midnight - 1_000), "2024-10-04");
        assert_eq!(instant_label(midnight, midnight + 1_000), "00:00:00 UTC");
    }

    #[test]
    fn an_unknown_state_is_not_live() {
        assert!(!RunState::Unknown.is_live());
        assert!(RunState::Starting.is_live());
        assert!(!RunState::Done.is_live());
    }

    fn machine() -> Machine {
        Machine {
            name: "zarathos-pc".to_string(),
            arch: "x86_64".to_string(),
            backend: "kvm".to_string(),
            irqchip: "ioapic".to_string(),
            class: "server".to_string(),
            cpus: 24,
            memory_mib: 128_320,
            vsock_mode: "uds-muxer".to_string(),
            started_at_ms: 1_000_000,
        }
    }

    #[test]
    fn showing_the_machine_fetches_it_and_says_its_uptime() {
        let mut model = Model::new();
        assert_eq!(model.apply(Message::Show(Pane::Machine)), Effect::machine());
        assert_eq!(model.pane, Pane::Machine);
        assert_eq!(model.machine_uptime_ms(), None, "nothing reported yet");

        model.apply(Message::Machine {
            at_ms: 1_003_000,
            result: Ok(machine()),
        });
        assert_eq!(model.machine.as_ref().expect("the machine").name, "zarathos-pc");
        assert_eq!(model.machine_uptime_ms(), Some(3_000), "ticking with the poll");
        assert_eq!(model.health(), Health::Live);
    }

    #[test]
    fn a_machine_the_daemon_does_not_serve_is_surfaced_not_swallowed() {
        let mut model = Model::new();
        model.apply(Message::Machine {
            at_ms: 5_000,
            result: Err(ApiError::Status {
                status: 404,
                message: "no such endpoint".to_string(),
            }),
        });
        assert_eq!(model.machine, None);
        assert_eq!(
            model.notice.as_ref().expect("a notice").text,
            "machine: 404: no such endpoint"
        );
        assert_eq!(model.health(), Health::Down);
    }

    #[test]
    fn a_tick_on_the_machine_pane_polls_the_machine() {
        let mut model = Model::new();
        model.apply(Message::Show(Pane::Machine));
        let effect = model.apply(Message::Tick { at_ms: 1_000 });
        assert!(effect.poll_machine, "{effect:?}");
        assert!(effect.poll_list, "the register is still polled for the tallies");
        assert!(effect.fetch_map.is_empty());
    }

    #[test]
    fn opening_the_map_asks_for_the_machine_and_every_runs_serial() {
        let mut model = Model::new();
        let mut r2 = run("r2", RunState::Killed);
        r2.vms[0].state = VmState::Killed;
        model.apply(Message::Runs {
            at_ms: 1_000,
            result: page(vec![run("r1", RunState::Running), r2]),
        });

        let effect = model.apply(Message::Show(Pane::Map));
        assert_eq!(model.pane, Pane::Map);
        assert!(effect.poll_machine, "{effect:?}");
        assert_eq!(effect.fetch_map, vec!["r1".to_string(), "r2".to_string()]);

        // A tick refreshes the same set, so the pulse measures against the
        // previous reading.
        let effect = model.apply(Message::Tick { at_ms: 4_000 });
        assert_eq!(effect.fetch_map, vec!["r1".to_string(), "r2".to_string()]);
        assert!(effect.poll_machine);
    }

    #[test]
    fn a_maps_serial_answers_feed_the_pulse() {
        let mut model = Model::new();
        model.apply(Message::Runs {
            at_ms: 1_000,
            result: page(vec![run("r1", RunState::Running)]),
        });
        let mut first = run("r1", RunState::Running);
        first.vms[0].serial = Some("login: plaza\n".to_string());
        model.apply(Message::Run {
            at_ms: 2_000,
            id: "r1".to_string(),
            result: Ok(first),
        });
        assert_eq!(model.activity.lps("r1", 0), 0.0, "the first sighting is a baseline");

        let mut second = run("r1", RunState::Running);
        second.vms[0].serial = Some("login: plaza\n$ uname\nLinux plaza\n".to_string());
        model.apply(Message::Run {
            at_ms: 4_000,
            id: "r1".to_string(),
            result: Ok(second),
        });
        assert!(
            model.activity.lps("r1", 0) > 0.0,
            "two lines in two seconds is a rate"
        );
    }

    #[test]
    fn a_run_that_left_the_register_stops_being_watched() {
        let mut model = Model::new();
        let mut live = run("r1", RunState::Running);
        live.vms[0].serial = Some("one\n".to_string());
        model.apply(Message::Runs {
            at_ms: 1_000,
            result: page(vec![run("r1", RunState::Running)]),
        });
        model.apply(Message::Run {
            at_ms: 2_000,
            id: "r1".to_string(),
            result: Ok(live),
        });
        assert!(!model.activity.is_empty());

        model.apply(Message::Runs {
            at_ms: 3_000,
            result: page(vec![]),
        });
        assert!(model.activity.is_empty());
    }

    #[test]
    fn pressing_a_map_cell_opens_that_run_with_that_vm_selected() {
        let mut model = Model::new();
        model.apply(Message::Show(Pane::Map));

        let effect = model.apply(Message::Select {
            run: "r3".to_string(),
            vm: 2,
        });
        assert_eq!(effect, Effect::fetch("r3"));
        assert_eq!(model.pane, Pane::Run("r3".to_string()));
        assert_eq!(
            model.selection,
            Some(Selection {
                run: "r3".to_string(),
                vm: 2
            })
        );
        assert_eq!(model.target_vm("r3"), 2);
        assert_eq!(model.target_vm("r9"), 0, "another run starts at its first VM");
    }

    #[test]
    fn opening_a_run_selects_its_first_vm_and_keeps_a_selection_within_it() {
        let mut model = Model::new();
        model.apply(Message::Select {
            run: "r1".to_string(),
            vm: 1,
        });
        // Back to the same run: the VM the operator chose is not reset.
        model.apply(Message::Show(Pane::Fleet));
        model.apply(Message::Show(Pane::Run("r1".to_string())));
        assert_eq!(model.target_vm("r1"), 1);

        // Another run starts at its own first VM.
        model.apply(Message::Show(Pane::Run("r2".to_string())));
        assert_eq!(model.target_vm("r2"), 0);
        assert_eq!(model.target_vm("r1"), 0, "r1 is no longer the selection");
    }

    #[test]
    fn a_vm_row_press_aims_the_console_at_that_vm() {
        let mut model = Model::new();
        model.apply(Message::Show(Pane::Run("r1".to_string())));
        assert!(model.apply(Message::SelectVm(1)).is_idle());
        assert_eq!(model.target_vm("r1"), 1);
        // Pressing a VM outside a run pane has nothing to target.
        model.apply(Message::Show(Pane::Fleet));
        model.apply(Message::SelectVm(3));
        assert_eq!(model.target_vm("r1"), 1, "unchanged");
    }

    #[test]
    fn sending_a_console_line_goes_to_the_selected_vm_newline_terminated() {
        let mut model = Model::new();
        model.apply(Message::Show(Pane::Run("r1".to_string())));
        model.apply(Message::SelectVm(1));
        model.apply(Message::ConsoleEdit("uname".to_string()));

        let effect = model.apply(Message::ConsoleSend);
        assert_eq!(
            effect,
            Effect::console(ConsoleCall {
                run: "r1".to_string(),
                vm: 1,
                data: "uname\n".to_string(),
            })
        );
        assert_eq!(model.console.line, "", "the box is cleared");
        assert_eq!(model.console.history, vec!["uname".to_string()]);
    }

    #[test]
    fn an_empty_console_line_and_a_console_without_a_run_send_nothing() {
        let mut model = Model::new();
        model.apply(Message::Show(Pane::Run("r1".to_string())));
        model.apply(Message::ConsoleEdit("   ".to_string()));
        assert!(model.apply(Message::ConsoleSend).is_idle());
        assert!(model.console.history.is_empty(), "nothing to remember");

        model.apply(Message::Show(Pane::Fleet));
        model.apply(Message::ConsoleEdit("uname".to_string()));
        assert!(model.apply(Message::ConsoleSend).is_idle());
        assert_eq!(model.console.line, "uname", "the box keeps what was typed");
    }

    #[test]
    fn a_quick_command_is_sent_as_a_line_and_remembered() {
        let mut model = Model::new();
        model.apply(Message::Show(Pane::Run("r1".to_string())));
        let effect = model.apply(Message::ConsoleQuick("poweroff"));
        assert_eq!(
            effect,
            Effect::console(ConsoleCall {
                run: "r1".to_string(),
                vm: 0,
                data: "poweroff\n".to_string(),
            })
        );
        assert_eq!(model.console.history, vec!["poweroff".to_string()]);
        assert_eq!(model.console.line, "");
    }

    #[test]
    fn the_history_arrows_walk_the_lines_and_stop_at_both_ends() {
        let mut model = Model::new();
        model.apply(Message::Show(Pane::Run("r1".to_string())));
        for line in ["help", "uname", "cpu"] {
            model.apply(Message::ConsoleEdit(line.to_string()));
            model.apply(Message::ConsoleSend);
        }

        let up = Message::ConsoleHistory(HistoryStep::Prev);
        assert!(model.apply(up.clone()).is_idle());
        assert_eq!(model.console.line, "cpu");
        model.apply(up.clone());
        assert_eq!(model.console.line, "uname");
        model.apply(up.clone());
        assert_eq!(model.console.line, "help");
        model.apply(up.clone());
        assert_eq!(model.console.line, "help", "the oldest entry is the end of the walk");

        let down = Message::ConsoleHistory(HistoryStep::Next);
        model.apply(down.clone());
        assert_eq!(model.console.line, "uname");
        model.apply(down.clone());
        assert_eq!(model.console.line, "cpu");
        model.apply(down);
        assert_eq!(model.console.line, "", "past the newest entry the box is empty");

        // Typing puts the box back in the operator's hands.
        model.apply(up);
        assert_eq!(model.console.line, "cpu");
        model.apply(Message::ConsoleEdit("mem".to_string()));
        assert_eq!(model.console.line, "mem");
    }

    #[test]
    fn a_history_arrow_outside_a_run_does_nothing() {
        let mut model = Model::new();
        model.apply(Message::ConsoleEdit("stale".to_string()));
        assert!(
            model
                .apply(Message::ConsoleHistory(HistoryStep::Prev))
                .is_idle()
        );
        assert_eq!(model.console.line, "stale");
    }

    #[test]
    fn the_history_is_bounded_and_does_not_repeat_the_last_line() {
        let mut console = Console::default();
        for n in 0..(HISTORY_DEPTH + 5) {
            console.remember(&format!("echo {n}"));
        }
        assert_eq!(console.history.len(), HISTORY_DEPTH);
        assert_eq!(
            console.history.last().expect("the newest"),
            &format!("echo {}", HISTORY_DEPTH + 4)
        );
        assert_eq!(console.history[0], format!("echo {}", 5));

        let last = console.history.last().expect("the newest").clone();
        console.remember(&last);
        assert_eq!(console.history.len(), HISTORY_DEPTH, "the same line twice is one entry");
    }

    #[test]
    fn a_console_answer_moves_the_transcript_on_and_answers_the_rate() {
        let mut model = Model::new();
        model.apply(Message::Show(Pane::Run("r1".to_string())));
        let mut first = run("r1", RunState::Running);
        first.vms[0].serial = Some("$ help\n".to_string());
        model.apply(Message::Input {
            at_ms: 2_000,
            id: "r1".to_string(),
            vm: 0,
            result: Ok(first),
        });

        let mut answered = run("r1", RunState::Running);
        answered.vms[0].serial = Some("$ help\ncommands: help uname cpu mem uptime\n".to_string());
        model.apply(Message::Input {
            at_ms: 4_000,
            id: "r1".to_string(),
            vm: 0,
            result: Ok(answered),
        });

        let detail = model.detail.as_ref().expect("the run");
        assert!(detail.serial().contains("commands: help"));
        assert_eq!(model.health(), Health::Live);
        assert!(
            model.activity.lps("r1", 0) > 0.0,
            "the reply grew the capture, and that growth is the rate"
        );
    }

    #[test]
    fn a_refused_console_line_keeps_the_daemons_words_and_the_selection() {
        let mut model = Model::new();
        model.apply(Message::Select {
            run: "r1".to_string(),
            vm: 1,
        });
        model.apply(Message::Input {
            at_ms: 4_000,
            id: "r1".to_string(),
            vm: 1,
            result: Err(ApiError::Status {
                status: 409,
                message: "the run is killed: console input needs a running run".to_string(),
            }),
        });
        let notice = model.notice.as_ref().expect("a notice");
        assert_eq!(notice.kind, NoticeKind::Failed);
        assert_eq!(
            notice.text,
            "console r1 vm 1: 409: the run is killed: console input needs a running run"
        );
        assert_eq!(model.target_vm("r1"), 1);
        assert_eq!(model.health(), Health::Down);
    }

    #[test]
    fn the_tallies_count_cids_vsock_runs_and_running_vms() {
        let mut model = Model::new();
        let mut with_vsock = run("r1", RunState::Running);
        with_vsock.vms = vec![
            vm(0, VmState::Running, Some(50), None),
            vm(1, VmState::Running, Some(51), None),
        ];
        let mut second = run("r2", RunState::Running);
        second.vms = vec![
            vm(0, VmState::Killed, Some(50), None),
            vm(1, VmState::Running, Some(52), None),
        ];
        let mut third = run("r3", RunState::Done);
        third.vms = vec![vm(0, VmState::Killed, None, None)];
        model.apply(Message::Runs {
            at_ms: 1_000,
            result: page(vec![with_vsock, second, third]),
        });

        assert_eq!(model.running_vms(), 3);
        assert_eq!(model.vsock_runs(), 2, "r3 boots without a vsock device");
        assert_eq!(
            model.guest_cids(),
            vec![50, 51, 52],
            "cid 50 appears twice and is counted once"
        );
        assert_eq!(
            tally_line(&model),
            "3 run(s) · 3 vm(s) up · 3 cid(s) · 2 vsock run(s)"
        );
    }

    #[test]
    fn the_latest_serial_line_skips_the_dangling_ones() {
        let model = Model::new();
        let mut vm = vm(0, VmState::Running, Some(50), Some("login: plaza\n$ uname\n\n"));
        assert_eq!(model.latest_line(&vm), Some("$ uname"));

        vm.serial = Some("login: plaza\n".to_string());
        assert_eq!(model.latest_line(&vm), Some("login: plaza"));

        vm.serial = None;
        assert_eq!(model.latest_line(&vm), None, "a list view has no serial");
        vm.serial = Some(String::new());
        assert_eq!(model.latest_line(&vm), None);
    }

    #[test]
    fn the_keys_show_each_pane_and_escape_comes_back() {
        let mut model = Model::new();
        assert_eq!(model.apply(Message::Key(KeyAction::Machine)), Effect::machine());
        assert_eq!(model.pane, Pane::Machine);
        assert_eq!(model.apply(Message::Key(KeyAction::Map)).poll_machine, true);
        assert_eq!(model.pane, Pane::Map);
        assert_eq!(model.apply(Message::Key(KeyAction::Register)), Effect::poll());
        assert_eq!(model.pane, Pane::Fleet);

        // Escape at the register is not an action: there is nothing behind it.
        assert!(model.apply(Message::Key(KeyAction::Back)).is_idle());
        model.apply(Message::Show(Pane::Machine));
        assert_eq!(model.apply(Message::Key(KeyAction::Back)), Effect::poll());
        assert_eq!(model.pane, Pane::Fleet);

        // The ledger's own key, which is the one read the sheet needs.
        assert_eq!(model.apply(Message::Key(KeyAction::Ledger)), Effect::ledger());
        assert_eq!(model.pane, Pane::Ledger);
        assert_eq!(model.apply(Message::Key(KeyAction::Roster)).poll_list, true);
        assert_eq!(model.pane, Pane::Roster);
    }

    #[test]
    fn the_history_keys_only_move_the_console_on_a_run() {
        let mut model = Model::new();
        model.console.history = vec!["help".to_string()];
        model.apply(Message::Show(Pane::Machine));
        assert!(
            model.apply(Message::Key(KeyAction::HistoryPrev)).is_idle()
        );
        assert_eq!(model.console.line, "", "the machine pane is not the console");

        model.apply(Message::Show(Pane::Run("r1".to_string())));
        model.apply(Message::Key(KeyAction::HistoryPrev));
        assert_eq!(model.console.line, "help");
        model.apply(Message::Key(KeyAction::HistoryNext));
        assert_eq!(model.console.line, "");
    }

    #[test]
    fn the_title_names_the_sheet_that_is_showing() {
        let mut model = Model::new();
        assert_eq!(title(&model), "AgentPlaza register — fleet");
        model.apply(Message::Show(Pane::Machine));
        assert_eq!(title(&model), "AgentPlaza register — machine");
        model.apply(Message::Show(Pane::Map));
        assert_eq!(title(&model), "AgentPlaza register — fleet map");
        model.apply(Message::Show(Pane::Telemetry));
        assert_eq!(title(&model), "AgentPlaza register — telemetry");
        model.apply(Message::Show(Pane::Ledger));
        assert_eq!(title(&model), "AgentPlaza register — ledger");
        model.apply(Message::Show(Pane::Spawn));
        assert_eq!(title(&model), "AgentPlaza register — new run");
        model.apply(Message::Show(Pane::Run("r7".to_string())));
        assert_eq!(title(&model), "AgentPlaza register — r7");
        model.apply(Message::Key(KeyAction::Back));
        assert_eq!(title(&model), "AgentPlaza register — fleet");
    }

    #[test]
    fn every_bar_left_the_register_stays_a_string() {
        assert_eq!(duration_label(0), "0s");
        assert_eq!(clock_label(0), "00:00:00 UTC");
    }

    #[test]
    fn every_health_has_a_label() {
        for health in [Health::Unknown, Health::Live, Health::Stale, Health::Down] {
            assert!(!health.label().is_empty());
        }
    }

    #[test]
    fn boot_polls_the_register_and_the_machine_once() {
        let mut model = Model::new();
        let effect = model.boot(1_000);
        assert!(effect.poll_list, "the register is polled");
        assert!(
            effect.poll_machine,
            "the machine is asked once, so the index's memory reading has the host's total"
        );
        assert!(effect.fetch_run.is_none(), "no pane is open at boot");
        assert_eq!(model.polls, 1);
        assert_eq!(model.now_ms, 1_000);

        // The panes' own ticks still decide for themselves.
        model.apply(Message::Runs {
            at_ms: 1_100,
            result: page(vec![]),
        });
        let effect = model.apply(Message::Tick { at_ms: 3_100 });
        assert!(effect.poll_list);
        assert!(!effect.poll_machine, "the register pane polls the list only");
    }

    #[test]
    fn pressing_a_preset_fills_the_form_and_submitting_sends_exactly_that() {
        let mut model = Model::new();
        // A form the operator had already typed into is replaced whole, and
        // the complaint about the old numbers goes with it.
        model.apply(Message::Edit {
            field: Field::N,
            value: "7".to_string(),
        });
        model.errors = vec![(Field::MemoryMib, "memory_mib must be at least 128".to_string())];

        assert!(
            model.apply(Message::Preset(Preset::Pair)).is_idle(),
            "a preset only fills the form; it boots nothing"
        );
        assert_eq!(model.form.field(Field::N), "2");
        assert_eq!(model.form.field(Field::Vcpus), "2");
        assert_eq!(model.form.field(Field::MemoryMib), "512");
        assert_eq!(model.form.field(Field::VsockCid), "10");
        assert!(model.errors.is_empty(), "{:?}", model.errors);
        assert_eq!(
            model.apply(Message::SpawnClicked),
            Effect {
                spawn: Some(CreateRun {
                    n: 2,
                    vcpus: 2,
                    memory_mib: 512,
                    beats: None,
                    vsock_cid: Some(10),
                }),
                ..Effect::none()
            }
        );

        // Another preset replaces the first one's numbers, including the cid
        // a run without vsock devices must not carry.
        model.apply(Message::Preset(Preset::Eight));
        assert_eq!(model.form.field(Field::VsockCid), "");
        assert_eq!(
            model.apply(Message::SpawnClicked),
            Effect {
                spawn: Some(CreateRun {
                    n: 8,
                    vcpus: 2,
                    memory_mib: 512,
                    beats: None,
                    vsock_cid: None,
                }),
                ..Effect::none()
            }
        );

        model.apply(Message::Preset(Preset::Scale));
        assert_eq!(
            model.apply(Message::SpawnClicked),
            Effect {
                spawn: Some(CreateRun {
                    n: 50,
                    vcpus: 1,
                    memory_mib: 128,
                    beats: None,
                    vsock_cid: None,
                }),
                ..Effect::none()
            }
        );
    }

    #[test]
    fn the_presets_footprint_is_the_memory_they_ask_the_host_for() {
        assert_eq!(preset_footprint(Preset::Pair), "2 × 512 MiB ≈ 1.0 GiB guest RAM");
        assert_eq!(preset_footprint(Preset::Eight), "8 × 512 MiB ≈ 4.0 GiB guest RAM");
        assert_eq!(preset_footprint(Preset::Scale), "50 × 128 MiB ≈ 6.3 GiB guest RAM");
    }

    #[test]
    fn memory_reads_in_mib_below_a_gibibyte_and_gibibytes_above_it() {
        assert_eq!(memory_label(0), "0 MiB");
        assert_eq!(memory_label(128), "128 MiB");
        assert_eq!(memory_label(1_023), "1023 MiB");
        assert_eq!(memory_label(1_024), "1.0 GiB");
        assert_eq!(memory_label(6_400), "6.3 GiB");
        assert_eq!(memory_label(128_320), "125.3 GiB");
        assert_eq!(memory_label(1_048_576), "1024.0 GiB");
    }

    #[test]
    fn the_register_reads_the_guest_memory_its_live_vms_have_committed() {
        let mut model = Model::new();
        assert_eq!(model.committed_mib(), 0);
        assert_eq!(model.committed_line(), "committed 0 MiB guest RAM");
        assert!(!model.overcommitted(), "no machine, no claim about the host");

        // One live VM at 512 MiB, and a killed run that commits nothing.
        let mut stopped = run("r2", RunState::Killed);
        stopped.vms[0].state = VmState::Killed;
        stopped.memory_mib = 4_096;
        model.apply(Message::Runs {
            at_ms: 1_000,
            result: page(vec![run("r1", RunState::Running), stopped]),
        });
        assert_eq!(model.committed_mib(), 512, "a killed VM does not hold memory");
        assert_eq!(model.committed_line(), "committed 512 MiB guest RAM");

        model.apply(Message::Machine {
            at_ms: 2_000,
            result: Ok(machine()),
        });
        assert_eq!(
            model.committed_line(),
            "committed 512 MiB guest RAM of 125.3 GiB"
        );
        assert!(!model.overcommitted());

        // A run that asks for more than the host has is named as such.
        let mut heavy = run("r3", RunState::Running);
        heavy.memory_mib = 200_000;
        model.apply(Message::Runs {
            at_ms: 3_000,
            result: page(vec![heavy]),
        });
        assert_eq!(model.committed_mib(), 200_000);
        assert!(model.overcommitted());
        assert_eq!(
            model.committed_line(),
            "committed 195.3 GiB guest RAM of 125.3 GiB · overcommit"
        );

        // Without the machine there is no total to be over.
        let mut unknown = model.clone();
        unknown.machine = None;
        assert!(!unknown.overcommitted());
        assert_eq!(unknown.committed_line(), "committed 195.3 GiB guest RAM");
    }

    /// A run that is open with a readable transcript.
    fn open_run(model: &mut Model) {
        model.apply(Message::Show(Pane::Run("r1".to_string())));
        let mut detail = run("r1", RunState::Running);
        detail.memory_mib = 512;
        detail.vms[0].serial = Some("login: plaza\n$ uname -a\nLinux\n".to_string());
        detail.vms.push(vm(1, VmState::Running, Some(51), Some("up\n")));
        model.apply(Message::Run {
            at_ms: 2_000,
            id: "r1".to_string(),
            result: Ok(detail),
        });
    }

    /// The path the register derives for the open run and the VM in hand.
    fn derived(run: &str, vm: u32) -> String {
        crate::export::export_path(
            &crate::export::default_dir(),
            run,
            vm,
            crate::export::ExportTarget::Vm,
        )
    }

    #[test]
    fn opening_a_run_fills_the_export_box_with_the_selected_vms_default_path() {
        let mut model = Model::new();
        assert_eq!(model.export.path, "", "nothing is derived before a run is open");

        model.apply(Message::Show(Pane::Run("r1".to_string())));
        assert_eq!(model.export.path, derived("r1", 0));
        assert!(model.export.path.ends_with("plaza-r1-0.txt"), "{}", model.export.path);

        open_run(&mut model);
        // Aiming the console at another VM moves the default with it.
        model.apply(Message::SelectVm(1));
        assert_eq!(model.export.path, derived("r1", 1));
    }

    #[test]
    fn an_export_before_the_run_has_been_read_says_so_instead_of_writing() {
        let mut model = Model::new();
        model.apply(Message::Show(Pane::Run("r1".to_string())));
        assert!(model.apply(Message::Export).is_idle());
        assert_eq!(
            model.notice.as_ref().expect("a notice").text,
            "export r1: the run has not been read yet"
        );
        assert_eq!(
            model.notice.as_ref().expect("a notice").kind,
            NoticeKind::Failed
        );
    }

    #[test]
    fn the_export_asks_for_the_selected_vms_transcript_at_the_path_in_the_box() {
        let mut model = Model::new();
        open_run(&mut model);

        let effect = model.apply(Message::Export);
        let call = effect.export.expect("an export was asked for");
        assert_eq!(call.run, "r1");
        assert_eq!(call.vm, Some(0));
        assert_eq!(call.path, derived("r1", 0));
        let detail = model.detail.clone().expect("the run");
        assert_eq!(
            call.contents,
            vm_transcript(&detail, &detail.vms[0], 2_000),
            "the bytes are the transcript, not a summary of it"
        );

        // All VMs, labelled, to the run's own default path.
        model.apply(Message::ExportTarget(ExportTarget::Run));
        assert!(model.export.path.ends_with("plaza-r1-all.txt"), "{}", model.export.path);
        let effect = model.apply(Message::Export);
        let call = effect.export.expect("an export was asked for");
        assert_eq!(call.vm, None);
        assert_eq!(call.contents, run_transcript(&detail, 2_000));
        assert!(call.contents.contains("# vm 1 · running · cid 51"), "{}", call.contents);
    }

    #[test]
    fn the_report_target_asks_for_the_runs_report_at_the_reports_own_path() {
        let mut model = Model::new();
        open_run(&mut model);

        model.apply(Message::ExportTarget(ExportTarget::Report));
        assert!(
            model.export.path.ends_with("plaza-r1-report.txt"),
            "{}",
            model.export.path
        );

        let effect = model.apply(Message::Export);
        let call = effect.export.expect("an export was asked for");
        assert_eq!(call.target, ExportTarget::Report);
        assert_eq!(call.run, "r1");
        assert_eq!(call.vm, None, "a report is the whole run, not one VM");
        let detail = model.detail.clone().expect("the run");
        assert_eq!(
            call.contents,
            run_report(&detail, 2_000),
            "the bytes are the report of the very run the pane holds"
        );
        assert!(call.contents.starts_with("# AgentPlaza report\n"), "{}", call.contents);
        // Neither VM's capture shows a session: the report says so rather than
        // claiming one.
        assert_eq!(
            call.contents.matches("session: none seen\n").count(),
            2,
            "{}",
            call.contents
        );

        // The notice names the report rather than "every vm".
        model.apply(Message::Exported {
            at_ms: 3_000,
            target: ExportTarget::Report,
            run: "r1".to_string(),
            vm: None,
            path: "/tmp/plaza-r1-report.txt".to_string(),
            result: Ok(128),
        });
        assert_eq!(
            model.notice.as_ref().expect("a notice").text,
            "exported 128 byte(s) of the report of run r1 to /tmp/plaza-r1-report.txt"
        );
    }

    #[test]
    fn a_path_the_operator_typed_is_kept_and_an_empty_box_gets_the_default_back() {
        let mut model = Model::new();
        open_run(&mut model);

        model.apply(Message::ExportEdited("/tmp/mine.txt".to_string()));
        assert!(model.export.edited);
        // Another VM, another target: a typed path is not second-guessed.
        model.apply(Message::SelectVm(1));
        model.apply(Message::ExportTarget(ExportTarget::Run));
        assert_eq!(model.export.to_write(), "/tmp/mine.txt");

        // Emptying the box hands it back to the register.
        model.apply(Message::ExportEdited("  ".to_string()));
        assert!(!model.export.edited);
        assert!(model.export.path.ends_with("plaza-r1-all.txt"), "{}", model.export.path);

        // And the guard the box cannot produce: a path that is not there is
        // refused rather than handed to `std::fs`.
        model.export.path.clear();
        assert_eq!(
            model.export_call().expect_err("no path"),
            "export: the path box is empty"
        );
        assert!(model.apply(Message::Export).is_idle());
        assert_eq!(
            model.notice.as_ref().expect("a notice").text,
            "export: the path box is empty"
        );
    }

    #[test]
    fn an_export_that_wrote_says_so_and_a_write_that_failed_is_surfaced() {
        let mut model = Model::new();
        open_run(&mut model);
        model.apply(Message::Exported {
            at_ms: 3_000,
            target: ExportTarget::Vm,
            run: "r1".to_string(),
            vm: Some(1),
            path: "/tmp/plaza-r1-1.txt".to_string(),
            result: Ok(64),
        });
        let notice = model.notice.as_ref().expect("a notice");
        assert_eq!(notice.kind, NoticeKind::Done);
        assert_eq!(
            notice.text,
            "exported 64 byte(s) of run r1 vm 1 to /tmp/plaza-r1-1.txt"
        );

        model.apply(Message::Exported {
            at_ms: 4_000,
            target: ExportTarget::Run,
            run: "r1".to_string(),
            vm: None,
            path: "/nope/plaza-r1-all.txt".to_string(),
            result: Err("No such file or directory (os error 2)".to_string()),
        });
        let notice = model.notice.as_ref().expect("a notice");
        assert_eq!(notice.kind, NoticeKind::Failed);
        assert_eq!(
            notice.text,
            "export /nope/plaza-r1-all.txt: No such file or directory (os error 2)"
        );
        // The daemon was never asked, so its connection is still whatever it
        // was: a local disk failure is not a daemon failure.
        assert_eq!(model.health(), Health::Live);
        assert_eq!(model.failures, 0);
    }

    #[test]
    fn the_help_key_opens_the_sheet_and_escape_or_the_key_closes_it() {
        let mut model = Model::new();
        assert!(!model.help);

        assert!(model.apply(Message::Key(KeyAction::Help)).is_idle());
        assert!(model.help);
        assert_eq!(model.pane, Pane::Fleet, "the sheet changes no pane");

        // While it is up, no key reaches a pane: the sheet is over the
        // register, not beside it.
        model.apply(Message::Key(KeyAction::Machine));
        assert_eq!(model.pane, Pane::Fleet);
        assert!(model.help);
        model.apply(Message::Key(KeyAction::Register));
        assert!(model.help, "r does not close it either");

        // `?` again, then Escape.
        model.apply(Message::Key(KeyAction::Help));
        assert!(!model.help);
        model.apply(Message::Key(KeyAction::Help));
        assert!(model.help);
        model.apply(Message::Key(KeyAction::Back));
        assert!(!model.help, "escape closes the sheet");

        // The console's arrows wait behind it like everything else.
        model.apply(Message::ConsoleEdit("stale".to_string()));
        model.apply(Message::Key(KeyAction::Help));
        model.apply(Message::Key(KeyAction::HistoryPrev));
        assert!(model.help);
        assert_eq!(model.console.line, "stale");

        // And the button is the same switch.
        assert!(model.apply(Message::Help(false)).is_idle());
        assert!(!model.help);
        assert!(model.apply(Message::Help(true)).is_idle());
        assert!(model.help);
    }

    #[test]
    fn escape_without_the_sheet_still_only_leaves_a_pane() {
        let mut model = Model::new();
        assert!(model.apply(Message::Key(KeyAction::Back)).is_idle());
        model.apply(Message::Show(Pane::Machine));
        assert_eq!(model.apply(Message::Key(KeyAction::Back)), Effect::poll());
        assert_eq!(model.pane, Pane::Fleet);
        assert!(!model.help);
    }

    /// One VM of a role fleet.
    fn role_vm(id: u32, role: Option<&str>, cid: Option<u64>, state: VmState) -> Vm {
        Vm {
            role: role.map(str::to_string),
            ..vm(id, state, cid, None)
        }
    }

    /// The daemon's initial setup as the register sees it: one run whose VMs
    /// each carry their role, next to an ordinary demo run whose VMs do not.
    fn fleet_with_roles() -> Model {
        let mut model = Model::new();
        let mut roles = run("r1", RunState::Running);
        roles.vcpus = 1;
        roles.memory_mib = 128;
        roles.vms = vec![
            role_vm(0, Some("builder"), Some(100), VmState::Running),
            role_vm(1, Some("reviewer"), Some(101), VmState::Running),
            role_vm(2, Some("builder"), Some(102), VmState::Exited),
            role_vm(3, None, Some(103), VmState::Running),
        ];
        model.apply(Message::Runs {
            at_ms: 10_000,
            result: page(vec![roles, run("r2", RunState::Done)]),
        });
        model
    }

    /// The register's half of a roster row, which a register row always has.
    fn reg(agent: &RosterAgent) -> &Registered {
        agent
            .registered
            .as_ref()
            .expect("the register reports this row")
    }

    /// A hub's `/status` body as a value: the CIDs it serves, and one row per
    /// connected agent — `(cid, role, connected_at_ms, last_heartbeat_ms,
    /// calls)`.
    fn hub_status(agents: &[(u32, &str, u64, u64, u64)]) -> HubStatus {
        HubStatus {
            node: crate::api::HubNode {
                cids: vec![10, 11, 12, 13, 14, 15],
                port: 5_000,
                // An hour and four minutes: the header's own `up 1h 04m`.
                uptime_s: 3_840,
                max_sessions: 64,
                keepalive_idle_s: 300,
            },
            agents: agents
                .iter()
                .map(
                    |&(cid, role, connected_at_ms, last_heartbeat_ms, calls)| HubAgent {
                        cid,
                        role: role.to_string(),
                        connected_at_ms,
                        last_heartbeat_ms,
                        calls,
                    },
                )
                .collect(),
            sessions_active: agents.len(),
        }
    }

    /// `fleet_with_roles` with a hub that has answered: the register's role
    /// agents and the hub's connected sessions, side by side.
    fn fleet_with_hub(agents: &[(u32, &str, u64, u64, u64)]) -> Model {
        let mut model = fleet_with_roles();
        model.hub = Some(Endpoint::loopback_at("127.0.0.1", 19_080).expect("loopback"));
        model.apply(Message::Hub {
            at_ms: 10_500,
            result: Ok(hub_status(agents)),
        });
        model
    }

    /// `fleet_with_roles` with the hub's admin surface configured: the roster
    /// carrying the operator's controls.
    fn fleet_with_admin() -> Model {
        fleet_with_roles().with_hub_admin(Some(
            Endpoint::loopback_at("127.0.0.1", 19_081).expect("loopback"),
        ))
    }

    /// The drive state the console is currently showing.
    fn drive(model: &Model) -> &DriveState {
        model
            .agent_console
            .as_ref()
            .expect("a console is open")
            .drive
            .as_ref()
            .expect("a drive has been made")
    }

    /// Fill the console's form and press send, returning the effect.
    fn drive_with(model: &mut Model, tool: &str, args: &str, timeout: &str) -> Effect {
        model.apply(Message::DriveEdit {
            field: ConsoleField::Tool,
            value: tool.to_string(),
        });
        model.apply(Message::DriveEdit {
            field: ConsoleField::Args,
            value: args.to_string(),
        });
        model.apply(Message::DriveEdit {
            field: ConsoleField::Timeout,
            value: timeout.to_string(),
        });
        model.apply(Message::DriveSend)
    }

    #[test]
    fn the_args_box_is_a_json_object_or_the_hubs_own_refusal() {
        assert_eq!(parse_args(""), Ok(serde_json::json!({})));
        assert_eq!(parse_args("   "), Ok(serde_json::json!({})));
        assert_eq!(
            parse_args(r#" {"path":"."} "#),
            Ok(serde_json::json!({"path": "."}))
        );
        for text in ["[1,2]", "5", "\"hi\"", "null", "true", "{oops"] {
            assert_eq!(
                parse_args(text),
                Err("args must be a JSON object".to_string()),
                "{text:?}"
            );
        }
    }

    #[test]
    fn the_operator_console_opens_on_the_row_the_operator_pressed() {
        let mut model = fleet_with_admin();
        assert!(model.has_hub_admin());
        assert!(model.agent_console.is_none(), "nothing is open until pressed");

        assert!(model.apply(Message::SelectAgent(100)).is_idle());
        let console = model.agent_console.as_ref().expect("open");
        assert_eq!(console.cid, 100);
        assert_eq!(console.tool, "");
        assert_eq!(console.args, "");
        assert_eq!(console.timeout, DEFAULT_TELL_TIMEOUT_MS.to_string());
        assert!(console.drive.is_none() && console.stop.is_none());

        // Pressing the same row again keeps the form the operator typed; a
        // different row replaces the console.
        model.apply(Message::DriveEdit {
            field: ConsoleField::Tool,
            value: "uptime".to_string(),
        });
        model.apply(Message::SelectAgent(100));
        assert_eq!(model.agent_console.as_ref().expect("kept").tool, "uptime");
        model.apply(Message::SelectAgent(101));
        let console = model.agent_console.as_ref().expect("moved");
        assert_eq!(console.cid, 101);
        assert_eq!(console.tool, "", "another agent is a fresh console");

        model.apply(Message::DriveClose);
        assert!(model.agent_console.is_none());
    }

    #[test]
    fn a_drive_is_validated_against_the_hubs_own_contract_before_it_is_sent() {
        let mut model = fleet_with_admin();
        model.apply(Message::SelectAgent(100));

        // An empty tool name.
        assert!(drive_with(&mut model, "", "", "5000").is_idle());
        assert_eq!(drive(&model).text(), Some("a tell needs a tool name"));
        assert_eq!(drive(&model).mark(), "NOT SENT");

        // A name past the hub's own bound.
        let long = "x".repeat(MAX_TOOL_NAME + 1);
        assert!(drive_with(&mut model, &long, "", "5000").is_idle());
        assert_eq!(
            drive(&model).text(),
            Some("the tool name is longer than 256 bytes")
        );

        // Args that are not a JSON object.
        assert!(drive_with(&mut model, "list", "[1,2]", "5000").is_idle());
        assert_eq!(drive(&model).text(), Some("args must be a JSON object"));

        // A timeout outside the hub's range (and one that is not a number).
        for timeout in ["0", "5001", "", "soon"] {
            assert!(drive_with(&mut model, "list", "{}", timeout).is_idle());
            assert_eq!(
                drive(&model).text(),
                Some("timeout_ms must be 1..=5000"),
                "{timeout:?}"
            );
        }
        // Nothing was ever sent: no call left the model on a bad form.
        assert!(matches!(drive(&model), DriveState::Rejected { .. }));
    }

    #[test]
    fn a_valid_drive_asks_for_the_call_with_the_parsed_arguments() {
        let mut model = fleet_with_admin();
        model.apply(Message::SelectAgent(100));

        let effect = drive_with(&mut model, "list", r#"{"path":"."}"#, "2000");
        let call = effect.tell_agent.expect("a drive was asked for");
        assert_eq!(call.cid, 100);
        assert_eq!(call.name, "list");
        assert_eq!(call.args, serde_json::json!({ "path": "." }));
        assert_eq!(call.timeout_ms, 2_000);
        assert!(matches!(
            drive(&model),
            DriveState::Driving { cid: 100, timeout_ms: 2_000, .. }
        ));

        // An empty args box is the empty object a no-argument tool wants, and
        // the tool name is trimmed.
        let effect = drive_with(&mut model, "  uptime  ", "", "5000");
        let call = effect.tell_agent.expect("a drive was asked for");
        assert_eq!(call.name, "uptime");
        assert_eq!(call.args, serde_json::json!({}));
        assert_eq!(call.timeout_ms, 5_000);
    }

    #[test]
    fn a_tell_answer_is_recorded_in_the_hubs_and_the_agents_own_words() {
        let mut model = fleet_with_admin();
        model.apply(Message::SelectAgent(100));

        // The agent answered: its `ok` and its output.
        model.apply(Message::Told {
            at_ms: 20_000,
            cid: 100,
            name: "uptime".to_string(),
            result: Ok(TellAnswer {
                ok: true,
                output: "3.20 3.13 1.00".to_string(),
            }),
        });
        assert_eq!(drive(&model).mark(), "ANSWERED");
        assert_eq!(drive(&model).text(), Some("3.20 3.13 1.00"));

        // A declining tool is an answer too: `ok:false` with its reason.
        model.apply(Message::Told {
            at_ms: 21_000,
            cid: 100,
            name: "read_file".to_string(),
            result: Ok(TellAnswer {
                ok: false,
                output: "denied: path \"../x\" contains \"..\"".to_string(),
            }),
        });
        assert_eq!(drive(&model).mark(), "TOOL DECLINED");
        assert_eq!(
            drive(&model).text(),
            Some("denied: path \"../x\" contains \"..\"")
        );

        // The hub's role policy, in its own words.
        model.apply(Message::Told {
            at_ms: 22_000,
            cid: 100,
            name: "list".to_string(),
            result: Err(ApiError::Status {
                status: 403,
                message: "role librarian does not grant list".to_string(),
            }),
        });
        assert_eq!(drive(&model).mark(), "ROLE POLICY");
        assert_eq!(drive(&model).text(), Some("role librarian does not grant list"));
        assert_eq!(drive(&model).meta(), "list · cid 100 · HTTP 403");

        // A served CID with no live session, and a silent agent.
        model.apply(Message::Told {
            at_ms: 23_000,
            cid: 100,
            name: "list".to_string(),
            result: Err(ApiError::Status {
                status: 409,
                message: "agent with guest CID 100 is not connected".to_string(),
            }),
        });
        assert_eq!(drive(&model).mark(), "REFUSED");
        model.apply(Message::Told {
            at_ms: 24_000,
            cid: 100,
            name: "list".to_string(),
            result: Err(ApiError::Status {
                status: 504,
                message: "agent did not answer within 1 ms".to_string(),
            }),
        });
        assert_eq!(drive(&model).text(), Some("agent did not answer within 1 ms"));

        // The admin surface could not be read at all.
        model.apply(Message::Told {
            at_ms: 25_000,
            cid: 100,
            name: "list".to_string(),
            result: Err(ApiError::Connect(
                "http://127.0.0.1:19081: refused".to_string(),
            )),
        });
        assert_eq!(drive(&model).mark(), "UNREACHABLE");
        // A hub refusal never marks the register down: the hub is a second
        // process on a second port.
        assert_eq!(model.failures, 0);
        assert_eq!(model.last_error, None);
    }

    #[test]
    fn a_tell_answer_for_a_console_that_moved_is_a_notice_not_a_loss() {
        let mut model = fleet_with_admin();
        model.apply(Message::SelectAgent(100));
        // The operator moves the console to another agent while the call is
        // in flight.
        model.apply(Message::SelectAgent(101));
        model.apply(Message::Told {
            at_ms: 20_000,
            cid: 100,
            name: "uptime".to_string(),
            result: Ok(TellAnswer {
                ok: true,
                output: "3.20 3.13".to_string(),
            }),
        });
        // The answer is not recorded on the console that moved away...
        assert!(model.agent_console.as_ref().expect("open").drive.is_none());
        // ...and it is not silently lost either.
        let notice = model.notice.as_ref().expect("a notice");
        assert_eq!(notice.kind, NoticeKind::Done);
        assert_eq!(notice.text, "cid 100 · 3.20 3.13");
    }

    #[test]
    fn stopping_takes_two_presses_and_opens_the_console_on_its_agent() {
        let mut model = fleet_with_admin();
        assert!(model.apply(Message::StopClicked(100)).is_idle());
        assert_eq!(model.armed_stop, Some(100));

        // Pressing another agent's stop moves the arm.
        assert!(model.apply(Message::StopClicked(101)).is_idle());
        assert_eq!(model.armed_stop, Some(101));

        let effect = model.apply(Message::StopClicked(101));
        assert_eq!(effect, Effect::stop(101));
        assert_eq!(model.armed_stop, None);
        let console = model.agent_console.as_ref().expect("the stop opened its console");
        assert_eq!(console.cid, 101);
        assert!(matches!(console.stop, Some(StopState::Stopping { cid: 101 })));
    }

    #[test]
    fn a_stop_outcome_is_recorded_honestly_and_tears_nothing_down() {
        let mut model = fleet_with_admin();
        model.apply(Message::SelectAgent(100));

        // The session ended, with the guest's own `bye` count.
        model.apply(Message::Stopped {
            at_ms: 20_000,
            cid: 100,
            result: Ok(StopOutcome::Stopped { calls: 3 }),
        });
        let state = model.agent_console.as_ref().expect("open").stop.clone().expect("a stop");
        assert_eq!(state.mark(), "STOPPED");
        assert_eq!(state.text(), "the session ended after 3 call(s); the agent's VM stays up");

        // The wait lapsed and the session is still up: the honest `not
        // stopped`, with the hub's own reason, and nothing torn down.
        model.apply(Message::Stopped {
            at_ms: 21_000,
            cid: 100,
            result: Ok(StopOutcome::StillUp {
                reason: "the agent did not end within 5000 ms".to_string(),
            }),
        });
        let state = model.agent_console.as_ref().expect("open").stop.clone().expect("a stop");
        assert_eq!(state.mark(), "NOT STOPPED");
        assert_eq!(
            state.text(),
            "not stopped: the agent did not end within 5000 ms — nothing was torn down"
        );

        // A CID the hub does not serve, and a served CID with no live session.
        for (status, message) in [
            (404u16, "no agent with guest CID 99"),
            (409, "agent with guest CID 100 is not connected"),
        ] {
            model.apply(Message::Stopped {
                at_ms: 22_000,
                cid: 100,
                result: Err(ApiError::Status {
                    status,
                    message: message.to_string(),
                }),
            });
            let state = model.agent_console.as_ref().expect("open").stop.clone().expect("a stop");
            assert_eq!(state.mark(), "REFUSED");
            assert_eq!(state.text(), message);
        }

        // And the admin surface being unreadable.
        model.apply(Message::Stopped {
            at_ms: 23_000,
            cid: 100,
            result: Err(ApiError::Connect("refused".to_string())),
        });
        let state = model.agent_console.as_ref().expect("open").stop.clone().expect("a stop");
        assert_eq!(state.mark(), "UNREACHABLE");
        assert_eq!(model.failures, 0, "a hub refusal is not a register failure");
    }

    #[test]
    fn leaving_the_roster_closes_the_operator_console_and_disarms_the_stop() {
        let mut model = fleet_with_admin();
        model.apply(Message::SelectAgent(100));
        model.apply(Message::StopClicked(100));
        assert_eq!(model.armed_stop, Some(100));
        assert!(model.agent_console.is_some());

        // Showing another sheet is leaving the roster: the console is the
        // roster's, and the arm goes with it.
        model.apply(Message::Show(Pane::Fleet));
        assert!(model.agent_console.is_none());
        assert_eq!(model.armed_stop, None);

        // Re-showing the roster does not resurrect it.
        model.apply(Message::Show(Pane::Roster));
        assert!(model.agent_console.is_none());
    }

    #[test]
    fn no_admin_surface_is_a_roster_with_no_controls() {
        let model = fleet_with_roles();
        assert!(!model.has_hub_admin());
        assert_eq!(model.hub_admin, None);
        // The console cannot be opened by the reducer either: nothing selects
        // an agent, and the roster draws no control to select one with.
        assert_eq!(model.agent_console, None);
    }

    #[test]
    fn the_roster_groups_role_agents_by_role_and_leaves_other_vms_out() {
        let model = fleet_with_roles();
        let groups = model.roster();

        // Two roles, in the order their VMs first appear in the register.
        assert_eq!(
            groups.iter().map(|group| group.role.as_str()).collect::<Vec<_>>(),
            vec!["builder", "reviewer"],
            "a role appears once, where it first appears"
        );

        let builder = &groups[0];
        assert_eq!(builder.agents.len(), 2, "both of the builder's VMs are agents");
        assert_eq!(builder.live(), 1, "one of them has stopped");
        assert_eq!(reg(&builder.agents[0]).run, "r1");
        assert_eq!(reg(&builder.agents[0]).vm, 0);
        assert_eq!(reg(&builder.agents[0]).guest_cid, Some(100));
        assert_eq!(reg(&builder.agents[0]).state, VmState::Running);
        assert!(builder.agents[0].is_live());
        assert_eq!(reg(&builder.agents[1]).state, VmState::Exited);
        assert!(!builder.agents[1].is_live());
        // With no hub configured there is no hub half to be judged by: every
        // row is the register's alone, and it is the sheet (not the row) that
        // knows the hub was never asked and so must not print a label.
        assert!(builder.agents.iter().all(|agent| !agent.is_connected()));
        assert!(
            builder.agents.iter().all(|agent| agent.missing_half() == Some("registered, not connected")),
            "the row's halves are the register's alone"
        );

        let reviewer = &groups[1];
        assert_eq!(reviewer.agents.len(), 1);
        assert_eq!(reviewer.live(), 1);

        // vm 3 carries no role, and the demo run's VM carries none either:
        // neither is a role agent, so neither is on the roster — only the two
        // role agents that are up are counted.
        assert_eq!(
            groups
                .iter()
                .flat_map(|group| group.agents.iter())
                .filter(|agent| agent.is_live())
                .count(),
            2
        );
        assert_eq!(model.roster_live(), 2, "builder vm 0 and reviewer vm 0");
        assert!(
            !groups
                .iter()
                .flat_map(|group| group.agents.iter())
                .any(|agent| reg(agent).vm == 3),
            "a VM with no role is not an agent, however it is running"
        );
    }

    #[test]
    fn a_register_with_no_role_agents_has_an_empty_roster() {
        let mut model = Model::new();
        model.apply(Message::Runs {
            at_ms: 10_000,
            result: page(vec![run("r1", RunState::Running), run("r2", RunState::Done)]),
        });
        assert!(
            model.roster().is_empty(),
            "a daemon that stamps no role has no roster to draw"
        );
        assert_eq!(model.roster_live(), 0);
        assert_eq!(
            roster_tally(&model.roster()),
            "0 agent(s) · 0 up right now · 0 role(s) · 0.0 line(s)/s"
        );
    }

    #[test]
    fn the_roster_tally_counts_the_agents_the_roles_and_their_rate() {
        let mut model = fleet_with_roles();
        model.activity.observe("r1", 0, 0, 10_000);
        model.activity.observe("r1", 0, 40, 12_000);
        let groups = model.roster();
        let tally = roster_tally(&groups);
        assert!(tally.starts_with("3 agent(s) · 2 up right now · 2 role(s) · "), "{tally}");
        assert!(
            !tally.contains("0.0 line(s)/s"),
            "the builder's rate is a real figure: {tally}"
        );
    }

    #[test]
    fn a_roster_agents_rate_is_the_activity_feed_s_own() {
        let mut model = fleet_with_roles();
        // Two reads of builder vm 0's capture 2 s apart, forty lines apart.
        model.activity.observe("r1", 0, 0, 10_000);
        model.activity.observe("r1", 0, 40, 12_000);
        let builder = &model.roster()[0];
        assert!(
            builder.agents[0].lps() > 0.0,
            "the roster's rate is the feed's, not a figure of its own"
        );
        assert_eq!(builder.lps(), builder.agents.iter().map(RosterAgent::lps).sum::<f32>());
        assert_eq!(
            builder.agents[1].lps(),
            0.0,
            "a VM nobody read a capture of yet has no rate"
        );
    }

    #[test]
    fn a_message_from_the_hub_is_the_connected_half_and_nothing_else() {
        let mut model = fleet_with_roles();
        assert!(!model.has_hub());
        assert_eq!(model.hub_connected(), None, "no hub was asked, so none answered");

        // The hub is configured and has answered: one builder connected.
        let mut model = model.with_hub(Some(
            Endpoint::loopback_at("127.0.0.1", 19_080).expect("loopback"),
        ));
        assert!(model.has_hub());
        model.apply(Message::Hub {
            at_ms: 10_500,
            result: Ok(hub_status(&[(100, "builder", 1_760_000_000_000, 1_760_000_009_000, 7)])),
        });
        assert_eq!(model.hub_connected(), Some(1));
        assert_eq!(model.hub_error, None);
        assert_eq!(model.hub_last_ok_ms, Some(10_500));

        // The join itself: the register's builder vm 0 and the hub's cid 100
        // are one row, carrying both halves.
        let builder = &model.roster()[0];
        assert_eq!(builder.role, "builder");
        let agent = &builder.agents[0];
        assert_eq!(reg(agent).vm, 0);
        assert_eq!(agent.cid(), Some(100));
        assert!(agent.is_registered() && agent.is_connected());
        assert_eq!(agent.missing_half(), None, "both halves are here");
        assert!(!agent.role_mismatch());
        assert_eq!(agent.role(), "builder");
        let connected = agent.connected.as_ref().expect("the hub's row");
        assert_eq!(connected.calls, 7);
        assert_eq!(agent.heartbeat_age_ms(1_760_000_012_000), Some(3_000));
        assert_eq!(builder.connected(), 1);
        // The register's other builder VM (cid 102, stopped) is not connected:
        // the join is per CID, not per role name.
        assert!(!builder.agents[1].is_connected());
    }

    #[test]
    fn a_register_agent_the_hub_reports_nobody_on_is_registered_not_connected() {
        // A hub that answered, and has nobody connected on any CID: the live
        // hub's own observation on a quiet host.
        let model = fleet_with_hub(&[]);
        assert_eq!(model.hub_connected(), Some(0));
        let groups = model.roster();
        assert_eq!(groups.len(), 2, "the register's roles are still the roster");
        for agent in groups.iter().flat_map(|group| group.agents.iter()) {
            assert!(agent.is_registered());
            assert!(!agent.is_connected());
            assert_eq!(agent.missing_half(), Some("registered, not connected"));
        }
        // The hub-only half is empty, so the tally is exactly the register's.
        assert_eq!(roster_tally(&groups), "3 agent(s) · 2 up right now · 2 role(s) · 0.0 line(s)/s");
    }

    #[test]
    fn a_connected_agent_the_register_does_not_hold_is_connected_not_registered() {
        // The hub accepted a session on a CID no register VM row claims, and
        // one the register does not report at all.
        let mut model = fleet_with_hub(&[
            (100, "builder", 1_760_000_000_000, 1_760_000_009_000, 7),
            (900, "librarian", 1_760_000_001_000, 1_760_000_011_000, 2),
        ]);
        // The register's last answer placed the whole fleet one poll ago; the
        // hub's answer is the newer one.
        assert_eq!(model.hub_connected(), Some(2));
        let groups = model.roster();

        let librarian = groups
            .iter()
            .find(|group| group.role == "librarian")
            .expect("the hub's role is on the sheet");
        assert_eq!(librarian.agents.len(), 1);
        let agent = &librarian.agents[0];
        assert!(!agent.is_registered(), "no VM row claims cid 900");
        assert!(agent.is_connected());
        assert_eq!(agent.registered, None);
        assert_eq!(agent.cid(), Some(900));
        assert_eq!(agent.role(), "librarian");
        assert_eq!(agent.missing_half(), Some("connected, not registered"));
        assert!(!agent.is_live(), "a session is not a running VM");
        assert_eq!(agent.lps(), 0.0, "no register half, no serial rate");
        assert_eq!(librarian.connected(), 1);
        assert_eq!(librarian.live(), 0);

        // The builder's matched row is not duplicated, and the roster's tally
        // counts the session the register cannot account for.
        assert_eq!(groups.iter().filter(|group| group.role == "builder").count(), 1);
        assert_eq!(
            roster_tally(&groups),
            "4 agent(s) · 2 up right now · 3 role(s) · 0.0 line(s)/s"
        );
    }

    #[test]
    fn a_role_the_hub_accepted_that_the_register_disagrees_with_is_a_mismatch() {
        // The same socket, two answers: the register booted cid 100 as a
        // builder, and the handshake it accepted says auditor.
        let mut model = fleet_with_hub(&[(100, "auditor", 1, 2, 0)]);
        let groups = model.roster();
        let builder = &groups[0];
        assert_eq!(builder.role, "builder", "the register's word files the row");
        let agent = &builder.agents[0];
        assert!(agent.role_mismatch());
        assert_eq!(agent.role(), "builder");
        assert_eq!(agent.connected.as_ref().expect("connected").role, "auditor");
        assert_eq!(agent.cid(), Some(100));
        // It is still one row with both halves — the disagreement is shown,
        // not used to split the agent in two.
        assert_eq!(agent.missing_half(), None);
        assert!(
            !groups.iter().any(|group| group.role == "auditor"),
            "the hub's word does not open a second group for the same socket"
        );

        // A mismatch does not touch the register's own health or failures: the
        // hub is a second process.
        model.apply(Message::Runs {
            at_ms: 11_000,
            result: page(vec![run("r1", RunState::Running)]),
        });
        assert_eq!(model.health(), Health::Live);
    }

    #[test]
    fn the_hub_is_read_on_its_own_schedule_and_never_moves_the_registers_health() {
        // No hub: the tick is exactly what it always was.
        let mut plain = Model::new();
        assert_eq!(plain.apply(Message::Tick { at_ms: 1_000 }), Effect::poll());
        assert!(!plain.apply(Message::Tick { at_ms: 100_000 }).poll_hub);

        // With a hub, the same tick asks for both: the register and the hub.
        let mut model = Model::new().with_hub(Some(
            Endpoint::loopback_at("127.0.0.1", 19_080).expect("loopback"),
        ));
        let effect = model.apply(Message::Tick { at_ms: 1_000 });
        assert_eq!(effect, Effect { poll_list: true, poll_hub: true, ..Effect::none() });
        assert_eq!(model.polls, 1, "the register's own count");
        assert_eq!(model.hub_next_at_ms, 3_000, "the hub's cadence is the register's");

        // A tick before the hub's schedule asks for neither part of it.
        assert!(model.apply(Message::Tick { at_ms: 2_000 }).is_idle());
        assert_eq!(model.hub_failures, 0);

        // The answers to that first tick's calls land, and both schedules
        // re-anchor on them — the register's in its own way, the hub's in its.
        model.apply(Message::Runs {
            at_ms: 1_000,
            result: page(vec![run("r1", RunState::Running)]),
        });
        let ok_health = model.health();
        assert_eq!(ok_health, Health::Live);
        model.apply(Message::Hub {
            at_ms: 1_000,
            result: Ok(hub_status(&[])),
        });
        assert_eq!(model.hub_next_at_ms, 3_000);
        assert_eq!(model.hub_connected(), Some(0), "nobody connected is an answer");

        // The next tick asks for both again.
        let effect = model.apply(Message::Tick { at_ms: 3_000 });
        assert!(effect.poll_list && effect.poll_hub, "{effect:?}");

        // The hub refuses. The register's health, failure count and schedule
        // are untouched: the hub is a second process, on a second port.
        let effect = model.apply(Message::Hub {
            at_ms: 3_100,
            result: Err(ApiError::Connect("http://127.0.0.1:19080: refused".to_string())),
        });
        assert!(effect.is_idle(), "an answer is not another call");
        assert_eq!(model.health(), ok_health, "the daemon is not down because the hub is");
        assert_eq!(model.failures, 0);
        assert_eq!(model.last_error, None);
        assert_eq!(model.hub_failures, 1);
        assert_eq!(
            model.hub_error.as_deref(),
            Some("cannot reach http://127.0.0.1:19080: refused")
        );
        assert_eq!(model.hub_status, None, "an unreachable hub has nothing to claim");
        assert_eq!(model.hub_next_at_ms, 7_100, "the hub backs off on its own failures");
        assert_eq!(model.polls, 2, "no hub message ever counts as a register poll");

        // And the register keeps answering for itself while the hub is down.
        model.apply(Message::Runs {
            at_ms: 3_200,
            result: page(vec![run("r1", RunState::Running)]),
        });
        assert_eq!(model.health(), Health::Live);
        assert_eq!(model.failures, 0);

        // The hub comes back: the error clears and the view is drawn again.
        let effect = model.apply(Message::Tick { at_ms: 7_100 });
        assert!(effect.poll_hub && effect.poll_list);
        model.apply(Message::Hub {
            at_ms: 7_200,
            result: Ok(hub_status(&[(100, "builder", 1, 2, 3)])),
        });
        assert_eq!(model.hub_error, None);
        assert_eq!(model.hub_failures, 0);
        assert_eq!(model.hub_connected(), Some(1));
        assert_eq!(model.hub_next_at_ms, 9_200, "the cadence measures from the answer");
    }

    #[test]
    fn the_roster_line_names_the_connected_count_only_when_a_hub_answered() {
        // No hub: the index's entry is the register's own count.
        let plain = fleet_with_roles();
        assert_eq!(roster_line(&plain), "2 role(s) · 2 agent(s) up");

        // A hub that answered: the entry carries the hub's own figure too, so
        // the index and the sheet cannot quietly disagree about who is
        // connected.
        let model = fleet_with_hub(&[
            (100, "builder", 1_760_000_000_000, 1_760_000_009_000, 7),
            (900, "librarian", 1_760_000_001_000, 1_760_000_011_000, 2),
        ]);
        assert_eq!(roster_line(&model), "3 role(s) · 2 agent(s) up · 2 connected");

        // A hub with nobody connected: nought is a figure, not an absence.
        let empty = fleet_with_hub(&[]);
        assert_eq!(roster_line(&empty), "2 role(s) · 2 agent(s) up · 0 connected");
    }

    #[test]
    fn the_hub_s_header_and_capacity_lines_say_what_the_surface_reports() {
        let status = hub_status(&[
            (10, "builder", 1_760_000_000_000, 1_760_000_009_000, 7),
            (11, "auditor", 1_760_000_001_000, 1_760_000_010_000, 2),
        ]);
        assert_eq!(
            hub_line(&status),
            "hub · 6 cid(s) served · 2 agent(s) connected · 2 session(s) active · up 1h 04m"
        );
        assert_eq!(
            hub_facts(&status),
            "serves cid 10..15 on vsock port 5000 · 2/64 session(s) · keepalive idle 300s"
        );

        // One CID is written as one, and none as none rather than a range.
        let mut single = status.clone();
        single.node.cids = vec![10];
        assert!(hub_facts(&single).starts_with("serves cid 10 on vsock port 5000"));
        single.node.cids = Vec::new();
        assert!(hub_facts(&single).starts_with("serves no cid on vsock port 5000"));

        // Every figure on the header comes from the field it names and no
        // other: the contract carries the session count *beside* the rows, so
        // a body where a registry row has gone while its session has not is
        // reported as exactly that, not as one number written twice.
        let mut uneven = status.clone();
        uneven.sessions_active = 5;
        assert_eq!(
            hub_line(&uneven),
            "hub · 6 cid(s) served · 2 agent(s) connected · 5 session(s) active · up 1h 04m"
        );
        let mut wide = status;
        wide.node.cids.push(16);
        wide.node.uptime_s = 0;
        assert_eq!(
            hub_line(&wide),
            "hub · 7 cid(s) served · 2 agent(s) connected · 2 session(s) active · up 0s"
        );
    }

    #[test]
    fn opening_the_roster_reads_the_register_and_every_runs_serial() {
        let mut model = Model::new();
        model.apply(Message::Runs {
            at_ms: 1_000,
            result: page(vec![run("r1", RunState::Running)]),
        });
        let effect = model.apply(Message::Show(Pane::Roster));
        assert_eq!(effect, Effect::roster(vec!["r1".to_string()]));
        assert_eq!(model.pane, Pane::Roster);
        assert!(
            !effect.poll_machine,
            "the roster draws no substrate, so it does not ask for the machine"
        );

        // The key is the same switch as the index entry.
        let mut model = fleet_with_roles();
        let effect = model.apply(Message::Key(KeyAction::Roster));
        assert_eq!(effect, Effect::roster(vec!["r1".to_string(), "r2".to_string()]));
        assert_eq!(model.pane, Pane::Roster);

        // With a hub configured, opening the roster asks for its status at
        // once: the pane's other half is a live connection list.
        let mut model = fleet_with_roles().with_hub(Some(
            Endpoint::loopback_at("127.0.0.1", 19_080).expect("loopback"),
        ));
        let effect = model.apply(Message::Show(Pane::Roster));
        assert_eq!(
            effect,
            Effect {
                poll_list: true,
                poll_hub: true,
                fetch_map: vec!["r1".to_string(), "r2".to_string()],
                ..Effect::none()
            }
        );
        assert!(!effect.poll_machine, "the roster still draws no substrate");
    }

    #[test]
    fn a_poll_that_failed_writes_no_observation() {
        let mut model = Model::new();

        // The poll never reached the daemon: nothing observed, nothing inked.
        model.apply(Message::Runs {
            at_ms: 1_000,
            result: Err(ApiError::Connect("refused".into())),
        });
        assert!(model.history.is_empty());
        assert_eq!(model.history.written(), 0);

        // The next poll answered: one observation, numbered from one.
        model.apply(Message::Runs {
            at_ms: 9_000,
            result: page(vec![run("r1", RunState::Running)]),
        });
        assert_eq!(model.history.len(), 1);
        assert_eq!(model.history.written(), 1);
        let sample = model.history.latest().expect("an observation");
        assert_eq!(sample.no, 1);
        assert_eq!(sample.ram_mib, 512);
        assert_eq!(sample.vms, 1);
    }

    #[test]
    fn the_telemetry_pane_watches_every_runs_serial_and_the_machine() {
        let mut model = Model::new();
        model.runs = vec![run("r1", RunState::Running), run("r2", RunState::Done)];
        model.apply(Message::Show(Pane::Telemetry));
        assert_eq!(model.pane, Pane::Telemetry);

        let effect = model.apply(Message::Tick { at_ms: 10_000 });
        assert!(effect.poll_list, "the list poll is the observation");
        assert!(effect.poll_machine, "the RAM chart takes the host's total");
        assert_eq!(effect.fetch_map, vec!["r1", "r2"], "the decomposition");
    }

    #[test]
    fn the_telemetry_key_shows_the_pane_and_escape_comes_back() {
        let mut model = Model::new();
        model.runs = vec![run("r1", RunState::Running)];
        let effect = model.apply(Message::Key(KeyAction::Telemetry));
        assert_eq!(model.pane, Pane::Telemetry);
        assert!(effect.poll_list, "opening the pane asks what there is to draw");
        assert_eq!(effect.fetch_map, vec!["r1"]);
        assert_eq!(title(&model), "AgentPlaza register — telemetry");
        model.apply(Message::Key(KeyAction::Back));
        assert_eq!(model.pane, Pane::Fleet);
    }

    #[test]
    fn a_pick_that_left_the_register_falls_back_to_the_whole_fleet() {
        let mut model = Model::new();
        model.runs = vec![run("r1", RunState::Running)];

        assert_eq!(
            model.apply(Message::TelemetryPick(Some("r1".to_string()))),
            Effect::none()
        );
        assert_eq!(model.telemetry_pick(), Some("r1"));

        // The pick is still remembered, but the charts fall back quietly.
        model.telemetry_run = Some("r9".to_string());
        assert_eq!(model.telemetry_pick(), None);
        assert_eq!(model.telemetry_run.as_deref(), Some("r9"));

        model.apply(Message::TelemetryPick(None));
        assert_eq!(model.telemetry_pick(), None);
        assert_eq!(model.telemetry_run, None);
    }

    #[test]
    fn the_run_panes_rings_hold_what_each_serial_observation_measured() {
        let mut model = Model::new();
        let first = run("r1", RunState::Running);
        model.apply(Message::Run {
            at_ms: 1_000,
            id: "r1".to_string(),
            result: Ok(Run {
                vms: vec![vm(0, VmState::Running, Some(50), Some("a\nb\n"))],
                ..first.clone()
            }),
        });
        let second = run("r1", RunState::Running);
        model.apply(Message::Run {
            at_ms: 3_000,
            id: "r1".to_string(),
            result: Ok(Run {
                vms: vec![vm(0, VmState::Running, Some(50), Some("a\nb\nc\nd\ne\nf\n"))],
                ..second
            }),
        });

        let series = model.history.vm_series("r1", 0);
        assert_eq!(series.len(), 2, "one point per observation");
        assert_eq!(series[0], 0.0, "the first sighting is a baseline");
        assert!(series[1] > 0.0, "the second measured a burst: {series:?}");
        assert!(model.history.vm_series("r1", 3).is_empty());
    }

    #[test]
    fn a_run_that_left_the_register_takes_its_rings_with_it() {
        let mut model = Model::new();
        model.apply(Message::Runs {
            at_ms: 1_000,
            result: page(vec![run("r1", RunState::Running)]),
        });
        let observed = run("r1", RunState::Running);
        model.apply(Message::Run {
            at_ms: 2_000,
            id: "r1".to_string(),
            result: Ok(Run {
                vms: vec![vm(0, VmState::Running, Some(50), Some("a\nb\n"))],
                ..observed
            }),
        });
        assert!(!model.history.vm_series("r1", 0).is_empty());

        model.apply(Message::Runs {
            at_ms: 3_000,
            result: page(vec![]),
        });
        assert!(model.history.vm_series("r1", 0).is_empty());
        assert_eq!(model.history.len(), 2, "the observations stay");
    }

    #[test]
    fn the_csv_box_is_stamped_where_the_session_started_and_a_typed_path_is_kept() {
        let mut model = Model::new();
        model.apply(Message::Runs {
            at_ms: 1_760_000_000_000,
            result: page(vec![]),
        });
        assert_eq!(
            model.csv.path,
            export::telemetry_path(&export::default_dir(), 1_760_000_000_000)
        );
        assert!(model.csv.path.ends_with(".csv"));

        model.apply(Message::CsvEdited("/tmp/plaza-session.csv".to_string()));
        assert!(model.csv.edited);
        model.apply(Message::Runs {
            at_ms: 1_760_000_002_000,
            result: page(vec![]),
        });
        assert_eq!(model.csv.path, "/tmp/plaza-session.csv", "a typed path is kept");

        // Emptying the box hands it back to the derived default.
        model.apply(Message::CsvEdited("   ".to_string()));
        assert!(!model.csv.edited);
        assert_eq!(
            model.csv.path,
            export::telemetry_path(&export::default_dir(), 1_760_000_000_000)
        );
    }

    #[test]
    fn the_csv_export_refuses_before_any_observation_and_writes_after_one() {
        let mut model = Model::new();
        let effect = model.apply(Message::CsvExport);
        assert!(effect.csv.is_none(), "there is nothing to write yet");
        assert_eq!(
            model.notice.as_ref().map(|notice| notice.kind),
            Some(NoticeKind::Failed)
        );
        assert!(
            model
                .notice
                .as_ref()
                .expect("a notice")
                .text
                .contains("no poll has observed the fleet"),
            "{:?}",
            model.notice
        );

        model.apply(Message::Runs {
            at_ms: 1_000,
            result: page(vec![run("r1", RunState::Running)]),
        });
        let call = model
            .apply(Message::CsvExport)
            .csv
            .expect("the session is written");
        assert_eq!(call.path, model.csv.path);
        assert_eq!(
            call.contents,
            crate::history::telemetry_csv(&model.history),
            "the file holds exactly the session's own observations"
        );

        // A write that lands is a done notice; one that cannot is the disk's.
        model.apply(Message::CsvExported {
            at_ms: 2_000,
            path: call.path.clone(),
            result: Ok(call.contents.len()),
        });
        assert_eq!(
            model.notice.as_ref().map(|notice| notice.kind),
            Some(NoticeKind::Done)
        );
        model.apply(Message::CsvExported {
            at_ms: 3_000,
            path: "/nope/plaza.csv".to_string(),
            result: Err("No such file or directory".to_string()),
        });
        let notice = model.notice.as_ref().expect("a notice");
        assert_eq!(notice.kind, NoticeKind::Failed);
        assert!(notice.text.contains("No such file or directory"));
    }

    #[test]
    fn a_missing_csv_path_is_a_refusal_rather_than_a_write() {
        let mut model = Model::new();
        model.apply(Message::Runs {
            at_ms: 1_000,
            result: page(vec![]),
        });
        model.apply(Message::CsvEdited("   ".to_string()));
        model.csv.path.clear();
        model.csv.edited = true;
        let effect = model.apply(Message::CsvExport);
        assert!(effect.csv.is_none());
        assert!(
            model
                .notice
                .as_ref()
                .expect("a notice")
                .text
                .contains("path box is empty"),
            "{:?}",
            model.notice
        );
    }

    /// A model whose register holds one running `r1`, with that run open on
    /// the pane and its capture read whole — the shape every serial event
    /// below is merged against.
    fn watching(capture: &str) -> Model {
        let mut model = Model::new();
        model.apply(Message::Runs {
            at_ms: 1_000,
            result: page(vec![run("r1", RunState::Running)]),
        });
        let mut detail = run("r1", RunState::Running);
        detail.vms[0].serial = Some(capture.to_string());
        model.apply(Message::Show(Pane::Run("r1".to_string())));
        model.apply(Message::Run {
            at_ms: 2_000,
            id: "r1".to_string(),
            result: Ok(detail),
        });
        model
    }

    #[test]
    fn an_event_moves_a_run_and_its_vms_in_place_without_a_poll() {
        let mut model = watching("login: plaza\n");
        let before = model.next_poll_at_ms;
        let polls = model.polls;

        let effect = model.apply(Message::Event {
            at_ms: 3_000,
            event: Event::Vm {
                run: "r1".to_string(),
                vm: 0,
                state: VmState::Exited,
            },
        });
        assert!(effect.is_idle(), "a known run's vm needs no call at all");
        assert_eq!(model.runs[0].vms[0].state, VmState::Exited);
        assert_eq!(
            model.detail.as_ref().expect("the open run").vms[0].state,
            VmState::Exited,
            "the pane reads the same run"
        );

        model.apply(Message::Event {
            at_ms: 3_100,
            event: Event::Run {
                id: "r1".to_string(),
                state: RunState::Done,
            },
        });
        assert_eq!(model.runs[0].state, RunState::Done);
        assert_eq!(
            model.detail.as_ref().expect("the open run").state,
            RunState::Done
        );

        // The stream is not the schedule: the poll's own clock is untouched.
        assert_eq!(model.next_poll_at_ms, before);
        assert_eq!(model.polls, polls, "the event did not count as a poll");
        assert_eq!(model.last_ok_ms, Some(3_100), "an event is the daemon answering");
    }

    #[test]
    fn an_event_naming_a_run_the_register_has_not_read_asks_for_it_once() {
        let mut model = Model::new();
        let first = model.apply(Message::Event {
            at_ms: 1_000,
            event: Event::Run {
                id: "r5".to_string(),
                state: RunState::Starting,
            },
        });
        assert!(first.poll_list, "the register is asked for the run it lacks");
        assert!(!first.is_idle());

        // A spawn publishes one event per VM as well: that must not be fifty
        // more polls.
        for _ in 0..50 {
            assert!(
                model
                    .apply(Message::Event {
                        at_ms: 1_010,
                        event: Event::Vm {
                            run: "r5".to_string(),
                            vm: 0,
                            state: VmState::Running,
                        },
                    })
                    .is_idle(),
                "one outstanding ask is enough"
            );
        }

        // The answer lands: the register holds the run, and a later event
        // moves it instead of asking again.
        model.apply(Message::Runs {
            at_ms: 1_020,
            result: page(vec![run("r5", RunState::Running)]),
        });
        let effect = model.apply(Message::Event {
            at_ms: 1_030,
            event: Event::Run {
                id: "r5".to_string(),
                state: RunState::Done,
            },
        });
        assert!(effect.is_idle());
        assert_eq!(model.runs[0].state, RunState::Done);

        // And an event for a run that is gone asks afresh.
        assert!(
            model
                .apply(Message::Event {
                    at_ms: 1_040,
                    event: Event::Run {
                        id: "r9".to_string(),
                        state: RunState::Running,
                    },
                })
                .poll_list
        );
    }

    #[test]
    fn an_event_that_wakes_an_ended_run_asks_the_register_instead_of_believing_it() {
        let mut model = watching("login: plaza\n");
        model.apply(Message::Event {
            at_ms: 3_000,
            event: Event::Run {
                id: "r1".to_string(),
                state: RunState::Killed,
            },
        });
        model.apply(Message::Event {
            at_ms: 3_010,
            event: Event::Vm {
                run: "r1".to_string(),
                vm: 0,
                state: VmState::Killed,
            },
        });
        assert_eq!(model.runs[0].state, RunState::Killed);

        // The same run never comes back. An event that says otherwise is
        // either older than the answer that settled it or the daemon has
        // begun its ids again: the register is asked, and its answer — which
        // carries each run's creation time — decides.
        let asks: Vec<bool> = [
            Event::Run {
                id: "r1".to_string(),
                state: RunState::Running,
            },
            Event::Run {
                id: "r1".to_string(),
                state: RunState::Starting,
            },
            Event::Vm {
                run: "r1".to_string(),
                vm: 0,
                state: VmState::Running,
            },
        ]
        .into_iter()
        .map(|event| model.apply(Message::Event { at_ms: 3_100, event }).poll_list)
        .collect();
        assert_eq!(
            asks,
            vec![true, false, false],
            "one outstanding ask is enough for a burst"
        );
        assert_eq!(model.runs[0].state, RunState::Killed, "a killed run stays killed");
        assert_eq!(model.runs[0].vms[0].state, VmState::Killed);
        assert_eq!(model.detail.as_ref().expect("the run").state, RunState::Killed);
    }

    #[test]
    fn a_list_answer_read_before_a_run_ended_does_not_walk_it_back() {
        let mut model = watching("");
        model.apply(Message::Event {
            at_ms: 3_000,
            event: Event::Run {
                id: "r1".to_string(),
                state: RunState::Done,
            },
        });

        // A poll whose answer was read before the run ended: still running,
        // and with no end time of its own.
        model.apply(Message::Runs {
            at_ms: 3_100,
            result: page(vec![run("r1", RunState::Running)]),
        });
        assert_eq!(model.runs[0].state, RunState::Done, "the register keeps what it saw");

        // And a list answer that agrees is taken as it comes.
        let mut ended = run("r1", RunState::Done);
        ended.ended_at = Some(3_050);
        model.apply(Message::Runs {
            at_ms: 3_200,
            result: page(vec![ended]),
        });
        assert_eq!(model.runs[0].state, RunState::Done);
        assert_eq!(model.runs[0].ended_at, Some(3_050));
    }

    #[test]
    fn a_serial_event_lands_in_the_transcript_between_polls() {
        let mut model = watching("login: plaza\n");
        assert_eq!(model.serial.length("r1", 0), 13);

        let effect = model.apply(Message::Event {
            at_ms: 3_000,
            event: Event::Serial {
                run: "r1".to_string(),
                vm: 0,
                offset: 13,
                text: "$ uname\nLinux plaza\n".to_string(),
            },
        });
        assert!(effect.is_idle(), "an append asks the daemon for nothing");
        assert_eq!(
            model.detail.as_ref().expect("the run").serial(),
            "login: plaza\n$ uname\nLinux plaza\n",
            "the console grew without a poll"
        );
        assert_eq!(model.serial.length("r1", 0), 33);

        // A replayed event, and a clip of one already partly held: neither
        // may duplicate a byte.
        for (offset, text) in [(0, "login: plaza\n"), (13, "$ uname\n")] {
            model.apply(Message::Event {
                at_ms: 3_100,
                event: Event::Serial {
                    run: "r1".to_string(),
                    vm: 0,
                    offset,
                    text: text.to_string(),
                },
            });
        }
        model.apply(Message::Event {
            at_ms: 3_200,
            event: Event::Serial {
                run: "r1".to_string(),
                vm: 0,
                offset: 13,
                text: "$ uname\nLinux plaza\n$ uptime\n".to_string(),
            },
        });
        assert_eq!(
            model.detail.as_ref().expect("the run").serial(),
            "login: plaza\n$ uname\nLinux plaza\n$ uptime\n",
            "the clip appended only the bytes that were new"
        );
    }

    #[test]
    fn the_transcript_the_stream_grew_is_the_capture_the_poll_reads() {
        // The daemon publishes its capture in deltas; the polls read it whole.
        // Merging the deltas must land on exactly the polled bytes.
        let whole = "login: plaza\n$ uname -a\nLinux plaza 7.2.8 x86_64\n$ uptime\n";
        let mut model = watching("");
        let deltas = [
            (0, "login: plaza\n"),
            (13, "$ uname -a\n"),
            (24, "Linux plaza 7.2.8"),
            (41, " x86_64\n"),
            (49, "$ uptime\n"),
        ];
        for (offset, text) in deltas {
            model.apply(Message::Event {
                at_ms: 3_000,
                event: Event::Serial {
                    run: "r1".to_string(),
                    vm: 0,
                    offset,
                    text: text.to_string(),
                },
            });
        }
        assert_eq!(model.detail.as_ref().expect("the run").serial(), whole);
        assert_eq!(model.serial.length("r1", 0), whole.len() as u64);

        // The poll's own capture is byte-for-byte what the stream built, and
        // applying it is not a change.
        let mut polled = run("r1", RunState::Running);
        polled.vms[0].serial = Some(whole.to_string());
        model.apply(Message::Runs {
            at_ms: 4_000,
            result: page(vec![polled.clone()]),
        });
        model.apply(Message::Run {
            at_ms: 4_100,
            id: "r1".to_string(),
            result: Ok(polled),
        });
        assert_eq!(model.detail.as_ref().expect("the run").serial(), whole);
        assert_eq!(model.serial.lines("r1", 0), Some(4));
    }

    #[test]
    fn a_gap_reads_the_open_run_again_and_a_quiet_one_not_at_all() {
        let mut model = watching("login: plaza\n");
        let effect = model.apply(Message::Event {
            at_ms: 3_000,
            event: Event::Serial {
                run: "r1".to_string(),
                vm: 0,
                offset: 900,
                text: "a stretch nobody saw\n".to_string(),
            },
        });
        assert_eq!(
            effect.fetch_run,
            Some("r1".to_string()),
            "the open run's capture is no longer contiguous: read it again"
        );
        assert_eq!(
            model.detail.as_ref().expect("the run").serial(),
            "login: plaza\n",
            "nothing was invented to fill the gap"
        );

        // The same gap for a run the register lists but is not showing: there
        // is no capture to repair, so nothing is asked for.
        let mut model = watching("");
        model.apply(Message::Show(Pane::Fleet));
        let effect = model.apply(Message::Event {
            at_ms: 3_000,
            event: Event::Serial {
                run: "r1".to_string(),
                vm: 0,
                offset: 900,
                text: "…".to_string(),
            },
        });
        assert!(effect.is_idle());
        assert_eq!(model.serial.length("r1", 0), 903);
    }

    #[test]
    fn a_serial_event_feeds_the_rate_the_register_draws() {
        // The poll's capture is the baseline; the events that follow move the
        // rate the map's pulse and the telemetry pane read.
        let mut model = watching("login: plaza\n");
        assert_eq!(model.activity.lps("r1", 0), 0.0, "one sighting is a baseline");

        model.apply(Message::Event {
            at_ms: 3_000,
            event: Event::Serial {
                run: "r1".to_string(),
                vm: 0,
                offset: 13,
                text: "$ uname\nLinux plaza\n".to_string(),
            },
        });
        assert!(
            model.activity.lps("r1", 0) > 0.0,
            "the stream's own growth is the rate, with no poll in between"
        );
        assert_eq!(
            model.history.vm_series("r1", 0).len(),
            2,
            "and it is in the VM's own ring"
        );

        // A run that is not being shown gets the same treatment: the map's
        // pulse is every run's serial.
        let mut model = Model::new();
        let mut live = run("r2", RunState::Running);
        live.vms[0].serial = Some("boot\n".to_string());
        model.apply(Message::Runs {
            at_ms: 1_000,
            result: page(vec![live.clone()]),
        });
        model.apply(Message::Run {
            at_ms: 2_000,
            id: "r2".to_string(),
            result: Ok(live),
        });
        model.apply(Message::Event {
            at_ms: 3_000,
            event: Event::Serial {
                run: "r2".to_string(),
                vm: 0,
                offset: 5,
                text: "more\n".to_string(),
            },
        });
        assert!(model.activity.lps("r2", 0) > 0.0);
        assert_eq!(model.detail, None, "and nothing was fetched to do it");
    }

    #[test]
    fn an_events_bytes_for_a_run_the_register_is_not_showing_stop_at_the_feed() {
        let mut model = watching("login: plaza\n");
        let open = model.detail.as_ref().expect("the run").serial().to_string();
        let mut other = run("r7", RunState::Running);
        other.vms[0].serial = Some("other\n".to_string());
        model.apply(Message::Runs {
            at_ms: 3_000,
            result: page(vec![
                run("r1", RunState::Running),
                other.clone(),
            ]),
        });

        model.apply(Message::Event {
            at_ms: 3_100,
            event: Event::Serial {
                run: "r7".to_string(),
                vm: 0,
                offset: 6,
                text: "more\n".to_string(),
            },
        });
        assert_eq!(
            model.detail.as_ref().expect("the run").serial(),
            open,
            "another run's bytes never reach the open transcript"
        );
        assert_eq!(model.serial.length("r7", 0), 11, "but they are tracked");
    }

    #[test]
    fn a_serial_feed_forgets_a_run_that_left_the_register() {
        let mut model = watching("login: plaza\n");
        assert!(!model.serial.is_empty());
        model.apply(Message::Runs {
            at_ms: 3_000,
            result: page(vec![]),
        });
        assert!(model.serial.is_empty(), "the run's captures went with it");
    }

    #[test]
    fn a_run_id_the_daemon_began_again_is_a_different_run() {
        // A daemon that restarts begins its run ids again: the register's r1
        // and the r1 the new daemon is reporting are different runs, and the
        // register must not read one into the other.
        let mut model = watching("login: plaza\n");
        assert_eq!(model.serial.length("r1", 0), 13);
        assert!(!model.activity.is_empty());

        // The old r1 ended.
        model.apply(Message::Event {
            at_ms: 2_500,
            event: Event::Run {
                id: "r1".to_string(),
                state: RunState::Done,
            },
        });

        // The new daemon's r1: same id, a creation time of its own.
        let mut reborn = run("r1", RunState::Running);
        reborn.created_at = 9_000_000;
        reborn.vms[0].serial = Some(String::new());
        model.apply(Message::Runs {
            at_ms: 3_000,
            result: page(vec![reborn]),
        });
        assert_eq!(
            model.runs[0].state,
            RunState::Running,
            "the new run is not held to the old one's ending"
        );
        assert!(model.serial.is_empty(), "the old run's captures went with it");
        assert!(model.activity.is_empty(), "and its rates");
        assert!(model.history.vm_series("r1", 0).is_empty(), "and its rings");

        // Its own bytes are then merged from the start, as any new capture is.
        model.apply(Message::Run {
            at_ms: 3_100,
            id: "r1".to_string(),
            result: Ok(model.runs[0].clone()),
        });
        model.apply(Message::Event {
            at_ms: 3_200,
            event: Event::Serial {
                run: "r1".to_string(),
                vm: 0,
                offset: 0,
                text: "login: plaza\n".to_string(),
            },
        });
        assert_eq!(
            model.detail.as_ref().expect("the run").serial(),
            "login: plaza\n"
        );
        assert_eq!(model.serial.length("r1", 0), 13);
    }

    #[test]
    fn an_event_keeps_the_stamp_fresh_without_hiding_a_refused_poll() {
        let mut model = watching("login: plaza\n");
        model.apply(Message::Runs {
            at_ms: 3_000,
            result: Err(ApiError::Connect("refused".to_string())),
        });
        assert_eq!(model.health(), Health::Down);

        model.apply(Message::Event {
            at_ms: 3_100,
            event: Event::Run {
                id: "r1".to_string(),
                state: RunState::Done,
            },
        });
        assert_eq!(
            model.health(),
            Health::Down,
            "a stream that still talks does not hide a poll the daemon refused"
        );
        assert_eq!(model.last_ok_ms, Some(3_100), "but the answer is recent");
    }

    #[test]
    fn an_unknown_state_from_the_stream_is_applied_as_unknown() {
        let mut model = watching("");
        model.apply(Message::Event {
            at_ms: 3_000,
            event: Event::Run {
                id: "r1".to_string(),
                state: RunState::Unknown,
            },
        });
        assert_eq!(model.runs[0].state, RunState::Unknown);
        // Unknown is not ended: a newer daemon's word for a run may still be
        // a live one, so it is not frozen.
        model.apply(Message::Event {
            at_ms: 3_100,
            event: Event::Run {
                id: "r1".to_string(),
                state: RunState::Running,
            },
        });
        assert_eq!(model.runs[0].state, RunState::Running);
    }

    // ---- the ledger: the plaza's tasks ----

    /// One task as the daemon writes it: every column present, and the ones a
    /// test cares about spelled out.
    fn task(id: &str, state: &str, assignee: Option<&str>, cid: Option<u64>) -> Task {
        let outcome = match state {
            "done" => Some("accepted"),
            "failed" => Some("failed"),
            "withdrawn" => Some("cancelled"),
            _ => None,
        };
        serde_json::from_value(serde_json::json!({
            "id": id,
            "goal": format!("the goal of {id}"),
            "acceptance": null,
            "budget": 0,
            "role": null,
            "state": state,
            "outcome": outcome,
            "assignee_id": assignee,
            "verifier_id": null,
            "agent_cid": cid,
            "note": null,
            "created_at": 1_000_000,
            "updated_at": 1_000_000,
        }))
        .expect("the fixture is the shape the daemon writes")
    }

    /// A ledger as the daemon lists it, fed to the model the way an answer is.
    fn ledger(tasks: Vec<Task>) -> Result<TasksPage, ApiError> {
        Ok(TasksPage { tasks })
    }

    /// A model whose ledger holds `tasks`, as of `at_ms`.
    fn with_ledger(tasks: Vec<Task>) -> Model {
        let mut model = Model::new();
        model.apply(Message::Show(Pane::Ledger));
        model.apply(Message::Tasks {
            at_ms: 10_000,
            result: ledger(tasks),
        });
        model
    }

    #[test]
    fn showing_the_ledger_reads_it_and_a_tick_while_it_is_up_keeps_reading_it() {
        let mut model = Model::new();
        let effect = model.apply(Message::Key(KeyAction::Ledger));
        assert_eq!(model.pane, Pane::Ledger);
        assert_eq!(effect, Effect::ledger(), "the sheet asks for the tasks");

        // While the sheet is up, the tick reads the ledger; the register is
        // still polled, and nothing else is asked for.
        model.apply(Message::Tasks {
            at_ms: 10_000,
            result: ledger(vec![task("t1", "open", None, None)]),
        });
        let effect = model.apply(Message::Tick { at_ms: 12_000 });
        assert!(effect.poll_tasks, "the ledger is read on the tick");
        assert!(effect.poll_list, "and so is the register");
        assert!(!effect.poll_machine && effect.fetch_map.is_empty());

        // Off the sheet, the ledger is not read at all.
        model.apply(Message::Show(Pane::Fleet));
        assert!(!model.apply(Message::Tick { at_ms: 14_000 }).poll_tasks);
    }

    #[test]
    fn the_ledger_fills_from_the_daemons_answer_and_counts_what_it_holds() {
        let mut model = Model::new();
        assert_eq!(
            task_line(&model),
            "not read yet",
            "a ledger nobody read is not an empty ledger"
        );
        model.apply(Message::Tasks {
            at_ms: 10_000,
            result: ledger(vec![
                task("t1", "open", None, None),
                task("t2", "claimed", Some("librarian"), Some(112)),
                task("t3", "done", Some("builder"), Some(100)),
            ]),
        });
        assert!(model.ledger_read);
        assert_eq!(model.tasks.len(), 3, "oldest first, as the daemon lists them");
        assert_eq!(
            task_line(&model),
            "3 task(s) · 1 open · 1 claimed · 1 ended"
        );
        assert_eq!(task_tally(&model.tasks), task_line(&model));
        assert_eq!(model.health(), Health::Live);

        // A failed read leaves the rows it already holds alone.
        model.apply(Message::Tasks {
            at_ms: 11_000,
            result: Err(ApiError::Connect("refused".to_string())),
        });
        assert_eq!(model.tasks.len(), 3);
        assert_eq!(model.health(), Health::Down);
    }

    #[test]
    fn a_task_event_patches_a_row_the_ledger_holds_and_invents_nothing() {
        let mut model = with_ledger(vec![task("t1", "open", None, None)]);
        let before = model.tasks[0].clone();

        assert!(model
            .apply(Message::Event {
                at_ms: 10_100,
                event: Event::Task {
                    id: "t1".to_string(),
                    state: TaskState::Claimed,
                    assignee: Some("librarian".to_string()),
                },
            })
            .is_idle());

        let held = &model.tasks[0];
        assert_eq!(held.state, TaskState::Claimed);
        assert_eq!(held.assignee_id.as_deref(), Some("librarian"));
        // What the frame does not carry is exactly what the poll wrote: a
        // delta may not invent a budget, a goal or a clock.
        assert_eq!(held.goal, before.goal);
        assert_eq!(held.budget, before.budget);
        assert_eq!(held.created_at, before.created_at);
        assert_eq!(held.updated_at, before.updated_at);
        assert_eq!(held.outcome.get(), None);
    }

    #[test]
    fn a_task_event_for_a_task_the_ledger_does_not_hold_asks_for_the_ledger_once() {
        let mut model = Model::new();
        let event = |at_ms: u64| Message::Event {
            at_ms,
            event: Event::Task {
                id: "t9".to_string(),
                state: TaskState::Open,
                assignee: None,
            },
        };
        assert_eq!(model.apply(event(10_000)), Effect::ledger());
        assert!(
            model.tasks.is_empty(),
            "a frame carries an id, a state and an assignee: it is not a row"
        );
        // The answer has not landed: a burst of events is still one read.
        assert!(model.apply(event(10_050)).is_idle());
        assert_eq!(model.ledger_asked, true);

        // The answer lands and holds t9: the next event is a patch, not a read,
        // and it never grows the ledger into a row nobody listed.
        model.apply(Message::Tasks {
            at_ms: 10_100,
            result: ledger(vec![task("t9", "open", None, None)]),
        });
        assert!(!model.ledger_asked);
        assert!(model.apply(event(10_200)).is_idle());
        assert_eq!(model.tasks[0].state, TaskState::Open);
        assert_eq!(model.tasks.len(), 1);

        // A task the answer did not carry asks again, and still invents nothing.
        assert_eq!(
            model.apply(Message::Event {
                at_ms: 10_300,
                event: Event::Task {
                    id: "t8".to_string(),
                    state: TaskState::Claimed,
                    assignee: Some("builder".to_string()),
                },
            }),
            Effect::ledger()
        );
        assert_eq!(model.tasks.len(), 1, "a frame is never a row");

        // And an event for a task it does hold, after the answer, is a patch.
        model.apply(Message::Tasks {
            at_ms: 10_400,
            result: ledger(vec![task("t9", "open", None, None)]),
        });
        assert!(model
            .apply(Message::Event {
                at_ms: 10_400,
                event: Event::Task {
                    id: "t9".to_string(),
                    state: TaskState::Withdrawn,
                    assignee: None,
                },
            })
            .is_idle());
        assert_eq!(model.tasks[0].state, TaskState::Withdrawn);
    }

    #[test]
    fn a_task_that_has_ended_is_never_walked_back_by_an_event() {
        let mut model = with_ledger(vec![task("t1", "claimed", Some("librarian"), Some(112))]);
        let done = |at_ms| Message::Event {
            at_ms,
            event: Event::Task {
                id: "t1".to_string(),
                state: TaskState::Done,
                assignee: Some("librarian".to_string()),
            },
        };
        model.apply(done(10_100));
        assert_eq!(model.tasks[0].state, TaskState::Done);

        // The same event again changes nothing at all.
        let settled = model.tasks[0].clone();
        model.apply(done(10_200));
        assert_eq!(model.tasks[0], settled, "a duplicate is a no-op");

        // And an event that would walk it back is not applied — the whole
        // frame, the assignee it carries included, is an older word.
        model.apply(Message::Event {
            at_ms: 10_300,
            event: Event::Task {
                id: "t1".to_string(),
                state: TaskState::Open,
                assignee: None,
            },
        });
        assert_eq!(model.tasks[0], settled);
        assert_eq!(model.tasks[0].assignee_id.as_deref(), Some("librarian"));
        // A state this console does not know is not a settled one for a task
        // that is still live — the runs' own rule — and it is not a way back
        // for one that has ended either: a settled task stays settled.
        let mut open = with_ledger(vec![task("t2", "claimed", Some("builder"), Some(100))]);
        open.apply(Message::Event {
            at_ms: 10_100,
            event: Event::Task {
                id: "t2".to_string(),
                state: TaskState::Unknown,
                assignee: Some("builder".to_string()),
            },
        });
        assert_eq!(open.tasks[0].state, TaskState::Unknown);
        open.apply(Message::Event {
            at_ms: 10_200,
            event: Event::Task {
                id: "t2".to_string(),
                state: TaskState::Done,
                assignee: Some("builder".to_string()),
            },
        });
        assert_eq!(open.tasks[0].state, TaskState::Done);
    }

    #[test]
    fn a_tasks_state_and_its_outcome_are_the_tasks_own_two_words() {
        // `done · accepted`, `failed · failed`, `withdrawn · cancelled`: the
        // state the daemon's ledger ends in, and the schema's `outcome` the
        // daemon recorded with it. The pair is read from the row, never
        // assumed: the three outcomes are three different words.
        for (state, outcome, line) in [
            ("done", Some("accepted"), "done · accepted"),
            ("failed", Some("failed"), "failed · failed"),
            ("withdrawn", Some("cancelled"), "withdrawn · cancelled"),
        ] {
            let mut task = task("t1", state, Some("builder"), Some(100));
            task.outcome = Column::filled(outcome.map(|outcome| {
                serde_json::from_value::<crate::api::TaskOutcome>(serde_json::json!(outcome))
                    .expect("the schema's own outcome")
            }));
            assert_eq!(task_state_line(&task), line, "the row's own pair");
        }
        // A task that is still live has no outcome to pair with.
        assert_eq!(task_state_line(&task("t2", "claimed", None, None)), "claimed");
        assert_eq!(task_state_line(&task("t3", "open", None, None)), "open");
    }

    #[test]
    fn a_ledger_answer_does_not_walk_a_settled_row_back() {
        // The events settled t1 as done; the list answer was read before that.
        let mut model = with_ledger(vec![task("t1", "claimed", Some("librarian"), Some(112))]);
        model.apply(Message::Event {
            at_ms: 10_100,
            event: Event::Task {
                id: "t1".to_string(),
                state: TaskState::Done,
                assignee: Some("librarian".to_string()),
            },
        });
        model.apply(Message::Tasks {
            at_ms: 10_200,
            result: ledger(vec![task("t1", "claimed", Some("librarian"), Some(112))]),
        });
        assert_eq!(
            model.tasks[0].state,
            TaskState::Done,
            "an older list keeps a settled task settled"
        );
        assert_eq!(model.tasks[0].assignee_id.as_deref(), Some("librarian"));

        // A task the daemon has begun again under a reused id inherits nothing.
        let mut page = task("t1", "open", None, None);
        page.created_at = 9_000_000;
        model.apply(Message::Tasks {
            at_ms: 10_300,
            result: ledger(vec![page]),
        });
        assert_eq!(model.tasks[0].state, TaskState::Open);
    }

    #[test]
    fn selecting_a_task_opens_its_detail_and_a_row_that_leaves_closes_it() {
        let mut model = with_ledger(vec![
            task("t1", "open", None, None),
            task("t2", "claimed", Some("builder"), Some(100)),
        ]);
        assert!(model.selected_task().is_none());
        assert!(model
            .apply(Message::SelectTask("t2".to_string()))
            .is_idle());
        assert_eq!(
            model.selected_task().map(|task| task.id.as_str()),
            Some("t2")
        );
        // The detail is the ledger's own row: a poll keeps it fresh.
        model.apply(Message::Tasks {
            at_ms: 10_100,
            result: ledger(vec![
                task("t1", "open", None, None),
                task("t2", "done", Some("builder"), Some(100)),
            ]),
        });
        assert_eq!(model.selected_task().expect("still held").state, TaskState::Done);

        // A selection the ledger no longer holds is dropped rather than drawn
        // from a task the daemon has forgotten.
        model.apply(Message::Tasks {
            at_ms: 10_200,
            result: ledger(vec![task("t1", "open", None, None)]),
        });
        assert!(model.task_selected.is_none());
        assert!(model.selected_task().is_none());
    }

    #[test]
    fn pressing_a_tasks_row_goes_through_the_ledger_sheet_and_opens_the_task() {
        // The same route a click takes: the pane's own key, then the row.
        let mut model = Model::new();
        model.apply(Message::Key(KeyAction::Ledger));
        model.apply(Message::Tasks {
            at_ms: 10_000,
            result: ledger(vec![
                task("t1", "open", None, None),
                task("t2", "claimed", Some("librarian"), Some(112)),
            ]),
        });
        model.apply(Message::SelectTask("t1".to_string()));
        assert_eq!(model.pane, Pane::Ledger);
        assert_eq!(model.selected_task().expect("the row").goal, "the goal of t1");
    }

    #[test]
    fn the_details_refresh_reads_the_one_task_the_ledger_has_open() {
        let mut model = with_ledger(vec![task("t1", "open", None, None)]);
        assert_eq!(
            model.apply(Message::TaskRefresh),
            Effect::none(),
            "nothing is open, so there is nothing to read"
        );
        let notice = model.notice.as_ref().expect("a refusal");
        assert_eq!(notice.kind, NoticeKind::Failed);
        assert_eq!(notice.text, "refresh: no task is open on the ledger");

        model.apply(Message::SelectTask("t1".to_string()));
        assert_eq!(
            model.apply(Message::TaskRefresh),
            Effect::task("t1"),
            "the open task is read again"
        );

        // The answer files what the daemon says, in place.
        let mut read = task("t1", "claimed", Some("librarian"), Some(112));
        read.note = crate::api::Column::filled(Some("taken".to_string()));
        model.apply(Message::Task {
            at_ms: 10_100,
            id: "t1".to_string(),
            result: Ok(read),
        });
        let held = &model.tasks[0];
        assert_eq!(held.state, TaskState::Claimed);
        assert_eq!(held.note.as_deref(), Some("taken"));
        assert_eq!(model.health(), Health::Live);

        // A 404 is an answer: the task is gone, and its detail closes.
        model.apply(Message::Task {
            at_ms: 10_200,
            id: "t1".to_string(),
            result: Err(ApiError::Status {
                status: 404,
                message: "no task with id t1".to_string(),
            }),
        });
        assert!(model.task_selected.is_none());
        let notice = model.notice.as_ref().expect("a notice");
        assert_eq!(notice.text, "task t1 is gone");
        assert_eq!(model.health(), Health::Live, "a 404 is not a dead daemon");
    }

    #[test]
    fn filing_a_task_validates_the_form_like_the_daemon() {
        let mut model = with_ledger(vec![]);
        // An empty goal is refused before anything is sent.
        assert!(model.apply(Message::TaskCreate).is_idle());
        assert_eq!(model.task_errors.len(), 1);
        let notice = model.notice.as_ref().expect("a refusal");
        assert_eq!(notice.kind, NoticeKind::Failed);
        assert_eq!(notice.text, "the form has 1 problem(s)");
        assert_eq!(
            TaskField::Goal.error(&model.task_errors),
            Some("goal must not be empty")
        );

        // Editing the field drops its complaint; a whole form is sent.
        model.apply(Message::TaskEdit {
            field: TaskField::Goal,
            value: "port the ledger".to_string(),
        });
        assert!(model.task_errors.is_empty());
        model.apply(Message::TaskEdit {
            field: TaskField::Budget,
            value: "500".to_string(),
        });
        let effect = model.apply(Message::TaskCreate);
        assert_eq!(
            effect.create_task,
            Some(CreateTask {
                goal: "port the ledger".to_string(),
                acceptance: None,
                budget: Some(500),
                role: None,
            })
        );
    }

    #[test]
    fn a_created_task_is_filed_selected_and_the_ledger_is_read_again() {
        let mut model = with_ledger(vec![task("t1", "open", None, None)]);
        let effect = model.apply(Message::TaskCreated {
            at_ms: 10_100,
            result: Ok(task("t2", "open", None, None)),
        });
        assert_eq!(model.tasks.len(), 2, "the ledger is oldest first");
        assert_eq!(model.tasks[1].id, "t2");
        assert_eq!(model.task_selected.as_deref(), Some("t2"));
        let notice = model.notice.as_ref().expect("a notice");
        assert_eq!(notice.kind, NoticeKind::Done);
        assert_eq!(notice.text, "task t2 created");
        assert!(
            effect.poll_tasks,
            "the ledger's own answer carries it in the daemon's order"
        );

        // A create the daemon refuses is its own words in the notice line.
        model.apply(Message::TaskCreated {
            at_ms: 10_200,
            result: Err(ApiError::Status {
                status: 400,
                message: "goal must be at most 4096 bytes".to_string(),
            }),
        });
        let notice = model.notice.as_ref().expect("a refusal");
        assert_eq!(notice.text, "task refused: 400: goal must be at most 4096 bytes");
    }

    #[test]
    fn claiming_takes_the_cid_in_the_box_and_only_from_an_open_task() {
        let mut model = with_ledger(vec![
            task("t1", "open", None, None),
            task("t2", "claimed", Some("builder"), Some(100)),
            task("t3", "done", Some("builder"), Some(100)),
        ]);

        // Nothing is open: the ledger refuses here rather than sending.
        assert!(model.apply(Message::TaskClaim).is_idle());
        assert_eq!(
            model.notice.as_ref().expect("a refusal").text,
            "claim: no task is open on the ledger"
        );

        // An open task and an empty CID box.
        model.apply(Message::SelectTask("t1".to_string()));
        model.apply(Message::ClaimCidEdit("  ".to_string()));
        assert!(model.apply(Message::TaskClaim).is_idle());
        assert_eq!(
            model.notice.as_ref().expect("a refusal").text,
            "claim: the agent CID box is empty"
        );

        // A CID that is not a number is refused the same way.
        model.apply(Message::ClaimCidEdit("cid 112".to_string()));
        assert!(model.apply(Message::TaskClaim).is_idle());
        assert_eq!(
            model.notice.as_ref().expect("a refusal").text,
            "claim: the agent CID must be a whole number"
        );

        // A whole number is the claim the daemon is asked for.
        model.apply(Message::ClaimCidEdit("112".to_string()));
        let effect = model.apply(Message::TaskClaim);
        assert_eq!(
            effect.claim_task,
            Some(ClaimCall {
                id: "t1".to_string(),
                agent_cid: 112,
            })
        );

        // A task already in hand cannot be claimed again: the daemon's own
        // state machine, said here without a round trip.
        model.apply(Message::SelectTask("t2".to_string()));
        assert!(model.apply(Message::TaskClaim).is_idle());
        assert_eq!(
            model.notice.as_ref().expect("a refusal").text,
            "task t2 is claimed: only an open task can be claimed"
        );
        model.apply(Message::SelectTask("t3".to_string()));
        assert!(model.apply(Message::TaskClaim).is_idle());
        assert_eq!(
            model.notice.as_ref().expect("a refusal").text,
            "task t3 is done: only an open task can be claimed"
        );
    }

    #[test]
    fn a_refused_claim_keeps_the_daemons_own_409_words() {
        let mut model = with_ledger(vec![task("t1", "open", None, None)]);
        model.apply(Message::SelectTask("t1".to_string()));
        model.apply(Message::ClaimCidEdit("4".to_string()));
        model.apply(Message::TaskClaim);

        // The daemon's answer, as it writes it: the message names the problem.
        model.apply(Message::TaskClaimed {
            at_ms: 10_100,
            id: "t1".to_string(),
            cid: 4,
            result: Err(ApiError::Status {
                status: 409,
                message: "no live agent has guest CID 4".to_string(),
            }),
        });
        let notice = model.notice.as_ref().expect("a refusal");
        assert_eq!(notice.kind, NoticeKind::Failed);
        assert_eq!(
            notice.text,
            "task t1 claim: 409: no live agent has guest CID 4",
            "the daemon's own words, verbatim"
        );
        assert_eq!(
            model.tasks[0].state,
            TaskState::Open,
            "a refused claim changed nothing"
        );
        assert_eq!(model.health(), Health::Down);

        // And an accepted one is the task the daemon answered with.
        model.apply(Message::TaskClaimed {
            at_ms: 10_200,
            id: "t1".to_string(),
            cid: 112,
            result: Ok(task("t1", "claimed", Some("librarian"), Some(112))),
        });
        assert_eq!(model.tasks[0].state, TaskState::Claimed);
        assert_eq!(model.tasks[0].agent_cid.copied(), Some(112));
        let notice = model.notice.as_ref().expect("a notice");
        assert_eq!(notice.kind, NoticeKind::Done);
        assert_eq!(notice.text, "task t1 claimed by cid 112");
    }

    #[test]
    fn finishing_carries_the_note_and_needs_a_claimed_task() {
        let mut model = with_ledger(vec![
            task("t1", "open", None, None),
            task("t2", "claimed", Some("librarian"), Some(112)),
        ]);
        // An open task cannot be finished, and nothing open is refused by name.
        assert!(model.apply(Message::TaskFinish(FinishOutcome::Done)).is_idle());
        assert_eq!(
            model.notice.as_ref().expect("a refusal").text,
            "finish: no task is open on the ledger"
        );
        model.apply(Message::SelectTask("t1".to_string()));
        assert!(model.apply(Message::TaskFinish(FinishOutcome::Done)).is_idle());
        assert_eq!(
            model.notice.as_ref().expect("a refusal").text,
            "task t1 is open: only a claimed task can be finished"
        );

        // A note the daemon would refuse is refused here, in its words.
        model.apply(Message::SelectTask("t2".to_string()));
        model.apply(Message::FinishNoteEdit("x".repeat(4_097)));
        assert!(model.apply(Message::TaskFinish(FinishOutcome::Done)).is_idle());
        assert_eq!(
            model.notice.as_ref().expect("a refusal").text,
            "note must be at most 4096 bytes"
        );

        // A claimed task, a note, and the outcome the operator pressed.
        model.apply(Message::FinishNoteEdit("21 handbooks committed".to_string()));
        let effect = model.apply(Message::TaskFinish(FinishOutcome::Done));
        assert_eq!(
            effect.finish_task,
            Some(FinishCall {
                id: "t2".to_string(),
                outcome: FinishOutcome::Done,
                note: Some("21 handbooks committed".to_string()),
            })
        );

        model.apply(Message::TaskFinished {
            at_ms: 10_100,
            id: "t2".to_string(),
            outcome: FinishOutcome::Done,
            result: Ok(task("t2", "done", Some("librarian"), Some(112))),
        });
        let held = &model.tasks[1];
        assert_eq!(held.state, TaskState::Done);
        assert_eq!(held.outcome.get(), Some(&crate::api::TaskOutcome::Accepted));
        assert_eq!(
            task_state_line(held),
            "done · accepted",
            "the daemon's state word and the schema's outcome are not the same word"
        );
        assert!(model.finish_note.is_empty(), "the note is on the task now");
        assert_eq!(
            model.notice.as_ref().expect("a notice").text,
            "task t2 finished done"
        );
        // A refused finish is the daemon's own words.
        model.apply(Message::TaskFinished {
            at_ms: 10_200,
            id: "t2".to_string(),
            outcome: FinishOutcome::Failed,
            result: Err(ApiError::Status {
                status: 409,
                message: "a task may not go from done to failed".to_string(),
            }),
        });
        assert_eq!(
            model.notice.as_ref().expect("a refusal").text,
            "task t2 finish: 409: a task may not go from done to failed"
        );
    }

    #[test]
    fn withdrawing_takes_two_presses_and_re_arms_on_another_task() {
        let mut model = with_ledger(vec![
            task("t1", "open", None, None),
            task("t2", "claimed", Some("builder"), Some(100)),
            task("t3", "done", Some("builder"), Some(100)),
        ]);
        assert!(model
            .apply(Message::TaskWithdrawClicked("t1".to_string()))
            .is_idle());
        assert_eq!(model.armed_task.as_deref(), Some("t1"));
        // Arming another task re-arms rather than withdrawing the first.
        assert!(model
            .apply(Message::TaskWithdrawClicked("t2".to_string()))
            .is_idle());
        assert_eq!(model.armed_task.as_deref(), Some("t2"));
        // The second press on the armed task is the withdrawal itself.
        assert_eq!(
            model.apply(Message::TaskWithdrawClicked("t2".to_string())),
            Effect {
                withdraw_task: Some("t2".to_string()),
                ..Effect::none()
            }
        );
        assert!(model.armed_task.is_none());

        model.apply(Message::TaskWithdrawn {
            at_ms: 10_100,
            id: "t2".to_string(),
            result: Ok(task("t2", "withdrawn", Some("builder"), Some(100))),
        });
        let held = &model.tasks[1];
        assert_eq!(held.state, TaskState::Withdrawn);
        assert_eq!(held.outcome.get(), Some(&crate::api::TaskOutcome::Cancelled));
        assert_eq!(
            model.notice.as_ref().expect("a notice").text,
            "task t2 withdrawn"
        );

        // Leaving the sheet disarms: the press was on the sheet left behind.
        model.apply(Message::TaskWithdrawClicked("t3".to_string()));
        model.apply(Message::Show(Pane::Fleet));
        assert!(model.armed_task.is_none());
    }

    #[test]
    fn the_ledger_offers_the_agents_a_claim_would_be_accepted_from() {
        // The register's running role VMs are exactly the CIDs the daemon
        // accepts a claim from; a VM with no role, and a stopped one, are not
        // agents.
        let model = fleet_with_roles();
        assert_eq!(
            model.live_agents(),
            vec![(100, "builder"), (101, "reviewer")],
            "running, and carrying a role"
        );
        // The register's third role VM is up but has no role, and its fourth
        // carries one but has stopped.
        let roles: Vec<&str> = model.live_agents().iter().map(|(_, role)| *role).collect();
        assert!(!roles.contains(&"nobody"));

        // No register at all, no agents to offer.
        let empty = Model::new();
        assert!(empty.live_agents().is_empty());
    }

    /// One artifact as `POST /api/v0/tasks/{id}/artifact` answers it: what the
    /// task's agent read back out of its workspace.
    fn artifact(task: &str, path: &str, content: &str) -> TaskArtifact {
        TaskArtifact {
            task: task.to_string(),
            agent_cid: 103,
            path: path.to_string(),
            content: content.to_string(),
            size: content.len() as u64,
            truncated: false,
        }
    }

    #[test]
    fn reading_an_artifact_row_asks_the_daemon_for_that_path_on_the_open_task() {
        let mut model = with_ledger(vec![task("t1", "claimed", Some("builder"), Some(103))]);
        model.apply(Message::SelectTask("t1".to_string()));
        assert_eq!(
            model.apply(Message::TaskArtifactRead("build-notes.txt".to_string())),
            Effect::artifact("t1", "build-notes.txt")
        );
        assert_eq!(
            model.artifact,
            Some(ArtifactState::Loading {
                task: "t1".to_string(),
                path: "build-notes.txt".to_string(),
            }),
            "the read is in flight until its answer lands"
        );

        // With no task open there is nothing to read, and the ledger says so
        // rather than sending a read against a task nobody selected.
        let mut closed = with_ledger(vec![task("t1", "claimed", Some("builder"), Some(103))]);
        assert!(closed
            .apply(Message::TaskArtifactRead("build-notes.txt".to_string()))
            .is_idle());
        assert_eq!(
            closed.notice.as_ref().expect("a refusal").text,
            "artifact: no task is open on the ledger"
        );
        assert!(closed.artifact.is_none(), "nothing was asked for");
    }

    #[test]
    fn an_artifacts_answer_is_the_agents_file_or_the_daemons_refusal() {
        // The agent read the file: the content and the terms it came in.
        let mut model = with_ledger(vec![task("t1", "done", Some("builder"), Some(103))]);
        model.apply(Message::SelectTask("t1".to_string()));
        let read = artifact("t1", "build-notes.txt", "task t1: verify\n");
        model.apply(Message::TaskArtifact {
            at_ms: 10_100,
            id: "t1".to_string(),
            path: "build-notes.txt".to_string(),
            result: Ok(read.clone()),
        });
        assert_eq!(
            model.artifact,
            Some(ArtifactState::Read {
                task: "t1".to_string(),
                artifact: read,
            })
        );

        // A refusal is the daemon answering: its own words, kept verbatim,
        // and the daemon is not marked down for a read the agent declined.
        model.apply(Message::TaskArtifact {
            at_ms: 10_200,
            id: "t1".to_string(),
            path: "build-notes.txt".to_string(),
            result: Err(ApiError::Status {
                status: 422,
                message: "denied: \"..\" is outside the workspace".to_string(),
            }),
        });
        match &model.artifact {
            Some(ArtifactState::Refused { path, error, .. }) => {
                assert_eq!(path, "build-notes.txt");
                assert_eq!(error.status(), Some(422));
                assert_eq!(error.to_string(), "422: denied: \"..\" is outside the workspace");
            }
            other => panic!("a refusal should be its own state: {other:?}"),
        }
        assert_eq!(model.failures, 0, "a refusal is an answer, not a failure");

        // A daemon that could not be reached *is* a failed call: the register
        // backs off and says so.
        model.apply(Message::TaskArtifact {
            at_ms: 10_300,
            id: "t1".to_string(),
            path: "build-notes.txt".to_string(),
            result: Err(ApiError::Connect("127.0.0.1:8899: refused".to_string())),
        });
        assert_eq!(model.failures, 1, "an unreachable daemon is a failure");
        assert!(matches!(model.artifact, Some(ArtifactState::Refused { .. })));
    }

    #[test]
    fn a_503_is_the_read_through_being_off_rather_than_a_refusal() {
        let mut model = with_ledger(vec![task("t1", "done", Some("builder"), Some(103))]);
        model.apply(Message::SelectTask("t1".to_string()));
        model.apply(Message::TaskArtifact {
            at_ms: 10_100,
            id: "t1".to_string(),
            path: "build-notes.txt".to_string(),
            result: Err(ApiError::Status {
                status: 503,
                message: "hub admin is not configured".to_string(),
            }),
        });
        assert_eq!(
            model.artifact,
            Some(ArtifactState::NotConfigured {
                task: "t1".to_string(),
                path: "build-notes.txt".to_string(),
                message: "hub admin is not configured".to_string(),
            }),
            "a read-through switched off is not the same fact as a refusal"
        );
        assert_eq!(model.failures, 0, "the daemon answered, and is up");
    }

    #[test]
    fn an_artifacts_answer_for_another_task_is_dropped() {
        let mut model = with_ledger(vec![
            task("t1", "done", Some("builder"), Some(103)),
            task("t2", "open", None, None),
        ]);
        model.apply(Message::SelectTask("t2".to_string()));

        // A read that raced the selection change answers for the task the
        // operator has left, and is not drawn under the one they opened.
        model.apply(Message::TaskArtifact {
            at_ms: 10_100,
            id: "t1".to_string(),
            path: "build-notes.txt".to_string(),
            result: Ok(artifact("t1", "build-notes.txt", "hi\n")),
        });
        assert!(model.artifact.is_none());
    }

    #[test]
    fn selecting_another_task_drops_a_read_of_the_first() {
        let mut model = with_ledger(vec![
            task("t1", "done", Some("builder"), Some(103)),
            task("t2", "done", Some("builder"), Some(103)),
        ]);
        model.apply(Message::SelectTask("t1".to_string()));
        model.apply(Message::TaskArtifactRead("build-notes.txt".to_string()));
        assert!(model.artifact.is_some());

        // Selecting the task the read belongs to keeps it; selecting another
        // drops it rather than leaving a read the detail no longer draws.
        model.apply(Message::SelectTask("t1".to_string()));
        assert!(model.artifact.is_some());
        model.apply(Message::SelectTask("t2".to_string()));
        assert!(model.artifact.is_none());
    }

    #[test]
    fn a_task_that_leaves_the_ledger_takes_its_artifact_with_it() {
        let mut model = with_ledger(vec![task("t1", "done", Some("builder"), Some(103))]);
        model.apply(Message::SelectTask("t1".to_string()));
        model.apply(Message::TaskArtifact {
            at_ms: 10_100,
            id: "t1".to_string(),
            path: "build-notes.txt".to_string(),
            result: Ok(artifact("t1", "build-notes.txt", "hi\n")),
        });
        assert!(model.artifact.is_some());

        // The ledger no longer holds t1: the selection and the read go with
        // it, rather than a read of a task the daemon has forgotten.
        model.apply(Message::Tasks {
            at_ms: 10_200,
            result: ledger(vec![task("t2", "open", None, None)]),
        });
        assert!(model.task_selected.is_none());
        assert!(model.artifact.is_none());
    }

    // ---- the treasury: the accounts and the journal ----

    /// The accounts of a one-cycle treasury: the mint, and the escrow a task's
    /// budget filled and its refund emptied.
    fn cycle_accounts() -> serde_json::Value {
        serde_json::json!([
            {"id":1,"kind":"mint","owner_ref":"mint","credit_limit":0,"balance":0,
             "created_at":1_000},
            {"id":2,"kind":"task_escrow","owner_ref":"t1","credit_limit":0,"balance":0,
             "created_at":1_000},
        ])
    }

    /// The journal's entries for that cycle: the mint, then the refund.
    fn cycle_entries() -> serde_json::Value {
        serde_json::json!([
            {"id":1,"kind":"mint","task_id":"t1","root_task":"t1","memo":"task budget",
             "posted_by":"treasury-service","created_at":1_000,"amount":750,
             "postings":[
                {"account":1,"kind":"mint","owner_ref":"mint","amount":-750},
                {"account":2,"kind":"task_escrow","owner_ref":"t1","amount":750}]},
            {"id":2,"kind":"refund","task_id":"t1","root_task":"t1",
             "memo":"task budget refunded","posted_by":"treasury-service",
             "created_at":2_000,"amount":750,
             "postings":[
                {"account":2,"kind":"task_escrow","owner_ref":"t1","amount":-750},
                {"account":1,"kind":"mint","owner_ref":"mint","amount":750}]},
        ])
    }

    /// A treasury read as the daemon answers one: the accounts with their
    /// totals, and the journal beside them.
    fn treasury_read(accounts: serde_json::Value, entries: serde_json::Value) -> TreasuryRead {
        let accounts_count = accounts.as_array().map_or(0, Vec::len);
        let entries_count = entries.as_array().map_or(0, Vec::len);
        let credits: i128 = entries
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.get("amount").and_then(serde_json::Value::as_i64))
            .map(i128::from)
            .sum();
        let treasury: crate::api::Treasury = serde_json::from_value(serde_json::json!({
            "accounts": accounts,
            "totals": {
                "accounts": accounts_count,
                "entries": entries_count,
                "credits": credits,
                "debits": credits,
                "balance": 0,
            },
            "spent": 0,
            "note": "v0 meters no real spend; a done task refunds its whole escrow",
        }))
        .expect("the fixture is the shape the daemon writes");
        let journal: Journal = serde_json::from_value(serde_json::json!({
            "entries": entries,
            "total": entries_count,
            "limit": 256,
            "truncated": false,
        }))
        .expect("the fixture is the shape the daemon writes");
        TreasuryRead { treasury, journal }
    }

    /// A model showing the treasury, with one read already answered.
    fn with_treasury(read: TreasuryRead) -> Model {
        let mut model = Model::new();
        model.apply(Message::Show(Pane::Treasury));
        model.apply(Message::Treasury {
            at_ms: 10_000,
            result: Ok(read),
        });
        model
    }

    #[test]
    fn showing_the_treasury_reads_it_and_a_tick_while_it_is_up_keeps_reading_it() {
        let mut model = Model::new();
        let effect = model.apply(Message::Key(KeyAction::Treasury));
        assert_eq!(model.pane, Pane::Treasury);
        assert_eq!(
            effect,
            Effect::treasury(),
            "the sheet asks for the accounts and the journal at once"
        );

        model.apply(Message::Treasury {
            at_ms: 10_000,
            result: Ok(treasury_read(cycle_accounts(), cycle_entries())),
        });
        assert!(model.treasury.is_some());
        assert_eq!(model.treasury_error, None);
        assert_eq!(model.treasury_last_ok_ms, Some(10_000));
        assert_eq!(
            model.treasury_next_at_ms, 12_000,
            "the treasury's cadence measures from its own answer"
        );

        // Not before the treasury's own schedule says so...
        assert!(!model.apply(Message::Tick { at_ms: 11_000 }).poll_treasury);
        // ...and the register is polled either way.
        let effect = model.apply(Message::Tick { at_ms: 13_000 });
        assert!(effect.poll_treasury, "the treasury is read on the tick");
        assert!(effect.poll_list, "and so is the register");

        // Off the sheet, the treasury is not read at all.
        model.apply(Message::Show(Pane::Fleet));
        assert!(!model.apply(Message::Tick { at_ms: 20_000 }).poll_treasury);
    }

    #[test]
    fn the_treasury_is_read_on_its_own_schedule_and_never_moves_the_registers_health() {
        let mut model = Model::new();
        // A console that never opened the sheet never reads the treasury.
        assert!(!model.apply(Message::Tick { at_ms: 1_000 }).poll_treasury);

        model.apply(Message::Show(Pane::Treasury));
        model.apply(Message::Runs {
            at_ms: 3_200,
            result: page(vec![run("r1", RunState::Running)]),
        });
        assert_eq!(model.health(), Health::Live);
        assert_eq!(model.failures, 0);

        // A treasury read fails: the failure is the treasury's own, so the
        // register stays live and un-backoff'd — an older daemon that serves
        // the register but not the treasury does not read down.
        model.apply(Message::Treasury {
            at_ms: 3_300,
            result: Err(ApiError::Connect("127.0.0.1:8899: refused".to_string())),
        });
        assert_eq!(model.treasury_failures, 1);
        assert!(model.treasury_error.is_some());
        assert_eq!(model.treasury, None, "nothing was read, so nothing is claimed");
        assert_eq!(model.failures, 0, "the register's failures are not the treasury's");
        assert_eq!(model.last_error, None);
        assert_eq!(model.health(), Health::Live);
        assert_eq!(
            model.treasury_next_at_ms,
            3_300 + poll_interval_ms(DEFAULT_REFRESH_MS, 1),
            "the treasury backs off on its own failures"
        );

        // It comes back: the error clears and the read is drawn.
        let effect = model.apply(Message::Tick { at_ms: 7_300 });
        assert!(effect.poll_treasury);
        model.apply(Message::Treasury {
            at_ms: 7_400,
            result: Ok(treasury_read(cycle_accounts(), cycle_entries())),
        });
        assert_eq!(model.treasury_error, None);
        assert_eq!(model.treasury_failures, 0);
        assert!(model.treasury.is_some());
        assert_eq!(model.treasury_next_at_ms, 9_400, "the cadence measures from the answer");
    }

    #[test]
    fn the_treasury_keeps_its_last_read_when_a_later_read_fails() {
        let mut model = with_treasury(treasury_read(cycle_accounts(), cycle_entries()));
        let held = model.treasury.clone();
        assert!(held.is_some());

        model.apply(Message::Treasury {
            at_ms: 12_000,
            result: Err(ApiError::Connect("refused".to_string())),
        });
        assert_eq!(
            model.treasury, held,
            "the journal is append-only: what was read stays true, only incomplete"
        );
        assert_eq!(model.treasury_last_ok_ms, Some(10_000), "the read's own age is kept");
        assert!(model.treasury_error.is_some(), "and the pane says it has not moved");
    }

    #[test]
    fn a_treasury_event_reads_the_journal_once_and_never_invents_an_entry() {
        let mut model = with_treasury(treasury_read(cycle_accounts(), cycle_entries()));
        let held: Vec<u64> = model
            .treasury
            .as_ref()
            .expect("a read")
            .journal
            .entries
            .iter()
            .map(|entry| entry.id)
            .collect();
        assert_eq!(held, vec![1, 2]);
        let before = model.treasury.clone();

        // A frame for an entry the pane already holds is one it has read:
        // a duplicate, and a no-op.
        assert_eq!(
            model.apply(Message::Event {
                at_ms: 13_000,
                event: Event::Treasury {
                    entry: 2,
                    kind: EntryKind::Refund,
                    amount: 750,
                    task: Some("t1".to_string()),
                },
            }),
            Effect::none()
        );
        assert_eq!(model.treasury, before, "no entry was invented from a frame");

        // An entry past the newest the pane holds means the journal moved on:
        // it is read again — and a burst is one read, not one per entry.
        assert_eq!(
            model.apply(Message::Event {
                at_ms: 13_100,
                event: Event::Treasury {
                    entry: 3,
                    kind: EntryKind::Mint,
                    amount: 5,
                    task: Some("t2".to_string()),
                },
            }),
            Effect::treasury()
        );
        assert!(model.treasury_asked);
        assert_eq!(
            model.apply(Message::Event {
                at_ms: 13_200,
                event: Event::Treasury {
                    entry: 4,
                    kind: EntryKind::Mint,
                    amount: 5,
                    task: None,
                },
            }),
            Effect::none(),
            "a burst of entries is one read"
        );

        // The answer lands and the ask is over.
        model.apply(Message::Treasury {
            at_ms: 13_300,
            result: Ok(treasury_read(cycle_accounts(), cycle_entries())),
        });
        assert!(!model.treasury_asked);

        // Off the sheet there is nothing to refresh, so a frame asks for
        // nothing: the sheet reads the journal when it opens.
        model.apply(Message::Show(Pane::Fleet));
        assert_eq!(
            model.apply(Message::Event {
                at_ms: 14_000,
                event: Event::Treasury {
                    entry: 9,
                    kind: EntryKind::Spend,
                    amount: 1,
                    task: None,
                },
            }),
            Effect::none()
        );
    }

    #[test]
    fn money_is_the_ledgers_unit_and_the_dollars_it_stands_for() {
        assert_eq!(money_label(1_500), "1500 cr · $1.50");
        assert_eq!(money_label(0), "0 cr · $0.00");
        assert_eq!(money_label(750), "750 cr · $0.75");
        // A debit, or an account below its limit, keeps its sign on both.
        assert_eq!(money_label(-750), "-750 cr · -$0.75");
        assert_eq!(money_label(-1), "-1 cr · -$0.00");
    }

    #[test]
    fn the_treasury_lines_name_what_the_sheet_draws() {
        let mut model = Model::new();
        assert_eq!(
            treasury_line(&model),
            "not read yet",
            "a treasury nobody read is not an empty treasury"
        );
        let read = treasury_read(cycle_accounts(), cycle_entries());
        model.apply(Message::Treasury {
            at_ms: 10_000,
            result: Ok(read.clone()),
        });
        assert_eq!(treasury_line(&model), "2 account(s) · 2 entry(s)");
        assert_eq!(
            totals_line(&read.treasury.totals),
            "2 account(s) · 2 entry(s) · credits 1500 cr · $1.50 · debits 1500 cr · $1.50 · \
             balance 0 cr · $0.00"
        );
        assert_eq!(
            journal_tally(&read.journal),
            "2 entry(s) carried of 2 in the journal"
        );
        // A journal cut to the daemon's limit says so rather than reading as
        // the whole thing.
        let mut cut = read.journal.clone();
        cut.total = 900;
        cut.truncated = true;
        assert_eq!(
            journal_tally(&cut),
            "2 entry(s) carried of 900 in the journal · cut to the newest 256"
        );

        // An entry's task, and its postings' accounts, as the rows write them.
        let mint = &read.journal.entries[0];
        assert_eq!(entry_task(mint), "t1");
        assert_eq!(posting_account(&mint.postings[0]), "mint · mint");
        assert_eq!(posting_account(&mint.postings[1]), "task_escrow · t1");
        // The daemon's own note, verbatim.
        assert_eq!(
            read.treasury.note,
            "v0 meters no real spend; a done task refunds its whole escrow"
        );
    }

    /// The board the daemon answers with after one call's bid-and-award cycle
    /// and one call still open: the calls, in the shape `GET /api/v0/agora`
    /// writes them.
    fn market_calls() -> serde_json::Value {
        serde_json::json!([
            {"task":"t1","capability":"builder","spec_ref":"the suite is green",
             "max_price":500,"state":"awarded","created_at":1_000,
             "bids":[
                {"task":"t1","agent_cid":100,"bidder":"builder","price":300,
                 "created_at":1_010},
                {"task":"t1","agent_cid":101,"bidder":"builder","price":250,
                 "created_at":1_020}],
             "award":{"task":"t1","contractor":"builder","agent_cid":100,"price":300,
                      "escrow_entry":1,"created_at":1_030}},
            {"task":"t2","capability":"reviewer","spec_ref":null,"max_price":90,
             "state":"open","created_at":1_100,"bids":[],"award":null},
        ])
    }

    /// A board read as the daemon answers one.
    fn board_of(calls: serde_json::Value) -> Board {
        let count = calls.as_array().map_or(0, Vec::len);
        serde_json::from_value(serde_json::json!({
            "calls": calls,
            "total": count,
            "limit": 256,
            "truncated": false,
        }))
        .expect("the fixture is the shape the daemon writes")
    }

    /// A model showing the market, with one read already answered.
    fn with_market(board: Board) -> Model {
        let mut model = Model::new();
        model.apply(Message::Show(Pane::Market));
        model.apply(Message::Agora {
            at_ms: 10_000,
            result: Ok(board),
        });
        model
    }

    #[test]
    fn showing_the_market_reads_it_and_a_tick_while_it_is_up_keeps_reading_it() {
        let mut model = Model::new();
        let effect = model.apply(Message::Key(KeyAction::Market));
        assert_eq!(model.pane, Pane::Market);
        assert_eq!(
            effect,
            Effect::agora(),
            "the sheet asks for the whole board at once"
        );

        model.apply(Message::Agora {
            at_ms: 10_000,
            result: Ok(board_of(market_calls())),
        });
        assert!(model.agora.is_some());
        assert_eq!(model.agora_error, None);
        assert_eq!(model.agora_last_ok_ms, Some(10_000));
        assert_eq!(
            model.agora_next_at_ms, 12_000,
            "the market's cadence measures from its own answer"
        );

        // Not before the market's own schedule says so...
        assert!(!model.apply(Message::Tick { at_ms: 11_000 }).poll_agora);
        // ...and the register is polled either way.
        let effect = model.apply(Message::Tick { at_ms: 13_000 });
        assert!(effect.poll_agora, "the market is read on the tick");
        assert!(effect.poll_list, "and so is the register");

        // Off the sheet, the market is not read at all.
        model.apply(Message::Show(Pane::Fleet));
        assert!(!model.apply(Message::Tick { at_ms: 20_000 }).poll_agora);
    }

    #[test]
    fn the_market_is_read_on_its_own_schedule_and_never_moves_the_registers_health() {
        let mut model = Model::new();
        // A console that never opened the sheet never reads the board.
        assert!(!model.apply(Message::Tick { at_ms: 1_000 }).poll_agora);

        model.apply(Message::Show(Pane::Market));
        model.apply(Message::Runs {
            at_ms: 3_200,
            result: page(vec![run("r1", RunState::Running)]),
        });
        assert_eq!(model.health(), Health::Live);
        assert_eq!(model.failures, 0);

        // A market read fails: the failure is the market's own, so the
        // register stays live and un-backoff'd — an older daemon that serves
        // the register but not the agora does not read down.
        model.apply(Message::Agora {
            at_ms: 3_300,
            result: Err(ApiError::Status {
                status: 404,
                message: "no such endpoint".to_string(),
            }),
        });
        assert_eq!(model.agora_failures, 1);
        assert!(model.agora_error.is_some());
        assert_eq!(model.agora, None, "nothing was read, so nothing is claimed");
        assert_eq!(
            model.failures, 0,
            "the register's failures are not the market's"
        );
        assert_eq!(model.last_error, None);
        assert_eq!(model.health(), Health::Live);
        assert_eq!(
            model.agora_next_at_ms,
            3_300 + poll_interval_ms(DEFAULT_REFRESH_MS, 1),
            "the market backs off on its own failures"
        );

        // It comes back: the error clears and the board is drawn.
        let effect = model.apply(Message::Tick { at_ms: 7_300 });
        assert!(effect.poll_agora);
        model.apply(Message::Agora {
            at_ms: 7_400,
            result: Ok(board_of(market_calls())),
        });
        assert_eq!(model.agora_error, None);
        assert_eq!(model.agora_failures, 0);
        assert!(model.agora.is_some());
        assert_eq!(
            model.agora_next_at_ms, 9_400,
            "the cadence measures from the answer"
        );
    }

    #[test]
    fn the_market_keeps_its_last_read_when_a_later_read_fails() {
        let mut model = with_market(board_of(market_calls()));
        let held = model.agora.clone();
        assert!(held.is_some());

        model.apply(Message::Agora {
            at_ms: 12_000,
            result: Err(ApiError::Connect("refused".to_string())),
        });
        assert_eq!(
            model.agora, held,
            "a bid the daemon recorded is a fact: what was read stays true"
        );
        assert_eq!(model.agora_last_ok_ms, Some(10_000), "the read's own age is kept");
        assert!(model.agora_error.is_some(), "and the pane says it has not moved");
    }

    #[test]
    fn an_agora_event_asks_for_the_board_once_and_never_invents_a_call() {
        let mut model = with_market(board_of(market_calls()));
        let before = model.agora.clone();

        // A frame the board already reflects is one it has read: a bid the
        // call holds from that agent at that price, and the award it holds.
        assert_eq!(
            model.apply(Message::Event {
                at_ms: 13_000,
                event: Event::Agora {
                    task: "t1".to_string(),
                    kind: AgoraKind::Bid,
                    agent_cid: 100,
                    price: 300,
                },
            }),
            Effect::none()
        );
        assert_eq!(
            model.apply(Message::Event {
                at_ms: 13_050,
                event: Event::Agora {
                    task: "t1".to_string(),
                    kind: AgoraKind::Award,
                    agent_cid: 100,
                    price: 300,
                },
            }),
            Effect::none()
        );
        assert_eq!(model.agora, before, "no call or bid was invented from a frame");

        // A bid the board does not hold (a new bidder on t2): the board is
        // read again — once, however many frames arrive at once.
        assert_eq!(
            model.apply(Message::Event {
                at_ms: 13_100,
                event: Event::Agora {
                    task: "t2".to_string(),
                    kind: AgoraKind::Bid,
                    agent_cid: 7,
                    price: 5,
                },
            }),
            Effect::agora()
        );
        assert!(model.agora_asked);
        assert_eq!(
            model.apply(Message::Event {
                at_ms: 13_200,
                event: Event::Agora {
                    task: "t2".to_string(),
                    kind: AgoraKind::Bid,
                    agent_cid: 8,
                    price: 6,
                },
            }),
            Effect::none(),
            "a burst of bids is one read"
        );

        // The answer lands and the ask is over; a re-bid at a price the board
        // does not hold is a fresh read.
        model.apply(Message::Agora {
            at_ms: 13_300,
            result: Ok(board_of(market_calls())),
        });
        assert!(!model.agora_asked);
        assert_eq!(
            model.apply(Message::Event {
                at_ms: 13_400,
                event: Event::Agora {
                    task: "t1".to_string(),
                    kind: AgoraKind::Bid,
                    agent_cid: 100,
                    price: 250,
                },
            }),
            Effect::agora(),
            "a re-bid at another price moved the board"
        );

        // Off the sheet there is nothing to refresh: the sheet reads the board
        // when it opens.
        model.apply(Message::Show(Pane::Fleet));
        assert_eq!(
            model.apply(Message::Event {
                at_ms: 14_000,
                event: Event::Agora {
                    task: "t9".to_string(),
                    kind: AgoraKind::Award,
                    agent_cid: 1,
                    price: 1,
                },
            }),
            Effect::none()
        );
    }

    #[test]
    fn the_market_lines_name_what_the_sheet_draws() {
        let mut model = Model::new();
        assert_eq!(
            market_line(&model),
            "not read yet",
            "a board nobody read is not an empty board"
        );
        let board = board_of(market_calls());
        model.apply(Message::Agora {
            at_ms: 10_000,
            result: Ok(board.clone()),
        });
        assert_eq!(market_line(&model), "2 call(s) · 2 bid(s)");
        assert_eq!(
            board_tally(&board),
            "2 call(s) carried of 2 · 2 bid(s) · 1 awarded"
        );
        // A board cut to the daemon's limit says so rather than reading as the
        // whole board.
        let mut cut = board.clone();
        cut.total = 900;
        cut.truncated = true;
        assert_eq!(
            board_tally(&cut),
            "2 call(s) carried of 900 · 2 bid(s) · 1 awarded · cut to the newest 256"
        );

        // An award names the treasury entry that funded it, labelled as the
        // treasury's own funding entry.
        let award = board.calls[0].award.get().expect("the award");
        assert_eq!(
            award_line(award),
            "awarded to builder · cid 100 · 300 cr · $0.30 · escrow entry 1 (the treasury's \
             funding entry)"
        );
        // A call whose task named no budget has no escrow entry, and says so.
        let unfunded = board_of(serde_json::json!([
            {"task":"t3","capability":"reviewer","spec_ref":null,"max_price":0,
             "state":"awarded","created_at":1,
             "bids":[{"task":"t3","agent_cid":201,"bidder":"cid:201","price":0,
                      "created_at":2}],
             "award":{"task":"t3","contractor":"cid:201","agent_cid":201,"price":0,
                      "escrow_entry":null,"created_at":3}}
        ]));
        assert_eq!(
            award_line(unfunded.calls[0].award.get().expect("the award")),
            "awarded to cid:201 · cid 201 · 0 cr · $0.00 · escrow entry none (a call whose task \
             named no budget)"
        );
        // The winning bid is the one the award names, not the higher one.
        let call = &board.calls[0];
        assert!(call.awarded_bid(&call.bids[0]));
        assert!(!call.awarded_bid(&call.bids[1]));
        // The open call's offer stamp is its own word.
        assert_eq!(board.calls[1].state.stamp_label(), "OFFERED");
    }
}
