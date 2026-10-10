//! The dashboard's headless core: the panes, the state, the reducer and the
//! poll arithmetic.
//!
//! Nothing here knows about iced, so the whole model — parsing outcomes, the
//! reducer, validation and the poll schedule — is unit-tested without a
//! display or a window. `main` turns an [`Effect`] into an iced [`Task`] and
//! nothing more.

use crate::api::{
    ApiError, CreateRun, Endpoint, HubAgent, HubStatus, Machine, Run, RunState, RunsPage, VmState,
};
use crate::events::Event;
use crate::export::{
    self, CsvCall, CsvExport, Export, ExportCall, ExportTarget, run_report, run_transcript,
    vm_transcript,
};
use crate::form::{Field, Preset, SpawnForm};
use crate::history::{self, SessionHistory};
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

/// What the reducer asks the runtime to do next.
///
/// Every field is an independent call, so the runtime can run them in any
/// order; the model never depends on which finished first.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
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
            | Message::Input { at_ms, .. }
            | Message::Killed { at_ms, .. }
            | Message::Spawned { at_ms, .. }
            | Message::Exported { at_ms, .. }
            | Message::CsvExported { at_ms, .. }
            | Message::Event { at_ms, .. } => Some(*at_ms),
            Message::Show(_)
            | Message::Key(_)
            | Message::Select { .. }
            | Message::SelectVm(_)
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
    /// How often a healthy daemon is polled, in milliseconds.
    pub refresh_ms: u64,
    /// The spawn form's raw input.
    pub form: SpawnForm,
    /// The spawn form's errors, from the last submission.
    pub errors: Vec<(Field, String)>,
    /// The run whose kill button was pressed once and is waiting for a second
    /// press.
    pub armed: Option<String>,
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
            refresh_ms: DEFAULT_REFRESH_MS,
            form: SpawnForm::default(),
            errors: Vec::new(),
            armed: None,
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
        }
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

    /// Shows `pane`, fetching what it needs.
    fn show(&mut self, pane: Pane) -> Effect {
        match pane {
            Pane::Fleet => {
                self.pane = Pane::Fleet;
                self.detail = None;
                self.armed = None;
                Effect::poll()
            }
            Pane::Run(id) => {
                self.armed = None;
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
                self.armed = None;
                self.errors.clear();
                Effect::none()
            }
            Pane::Machine => {
                self.pane = Pane::Machine;
                self.detail = None;
                self.armed = None;
                Effect::machine()
            }
            Pane::Map => {
                self.pane = Pane::Map;
                self.detail = None;
                self.armed = None;
                // The plate is labelled with the machine's model and its pulse
                // is measured from every run's serial, so opening it asks for
                // both at once.
                Effect::map(self.run_ids())
            }
            Pane::Roster => {
                self.pane = Pane::Roster;
                self.detail = None;
                self.armed = None;
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
            Pane::Telemetry => {
                self.pane = Pane::Telemetry;
                self.detail = None;
                self.armed = None;
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
    use crate::api::Vm;

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
}
