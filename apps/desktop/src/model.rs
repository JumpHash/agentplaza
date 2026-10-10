//! The dashboard's headless core: the panes, the state, the reducer and the
//! poll arithmetic.
//!
//! Nothing here knows about iced, so the whole model — parsing outcomes, the
//! reducer, validation and the poll schedule — is unit-tested without a
//! display or a window. `main` turns an [`Effect`] into an iced [`Task`] and
//! nothing more.

use crate::api::{ApiError, CreateRun, Endpoint, Machine, Run, RunsPage};
use crate::form::{Field, SpawnForm};
use crate::map::Activity;

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
    /// Escape: back to the register.
    Back,
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
    /// Fetch `GET /api/v0/runs/{id}`.
    pub fetch_run: Option<String>,
    /// Fetch each of these runs, with serial: the map's activity feed.
    pub fetch_map: Vec<String>,
    /// `DELETE /api/v0/runs/{id}`.
    pub kill: Option<String>,
    /// `POST /api/v0/runs` with this body.
    pub spawn: Option<CreateRun>,
    /// `POST /api/v0/runs/{id}/input` with this console write.
    pub input: Option<ConsoleCall>,
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
    /// Dismiss the notice line.
    DismissNotice,
}

impl Message {
    /// The wall-clock time this message carries, when it carries one.
    pub fn at_ms(&self) -> Option<u64> {
        match self {
            Message::Tick { at_ms }
            | Message::Runs { at_ms, .. }
            | Message::Run { at_ms, .. }
            | Message::Machine { at_ms, .. }
            | Message::Input { at_ms, .. }
            | Message::Killed { at_ms, .. }
            | Message::Spawned { at_ms, .. } => Some(*at_ms),
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
            | Message::DismissNotice => None,
        }
    }
}

/// The dashboard's state.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    /// The daemon this register is talking to.
    pub endpoint: Endpoint,
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
    /// Every VM's serial rate, from the deltas between polls.
    pub activity: Activity,
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
            pane: Pane::Fleet,
            runs: Vec::new(),
            detail: None,
            machine: None,
            follow: true,
            selection: None,
            console: Console::default(),
            activity: Activity::default(),
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

    /// How long a run has been up: created to ended, or created to now.
    pub fn run_elapsed_ms(&self, run: &Run) -> u64 {
        run.ended_at
            .unwrap_or(self.now_ms)
            .saturating_sub(run.created_at)
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
                        self.runs = page.runs;
                        self.activity.retain(&self.runs);
                        self.succeeded(at_ms);
                    }
                    Err(error) => self.failed(at_ms, &error),
                }
                Effect::none()
            }
            Message::Run { at_ms, id, result } => self.run_answer(at_ms, &id, result),
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
            Message::DismissNotice => {
                self.notice = None;
                Effect::none()
            }
        }
    }

    /// The timer fired: poll if the schedule says so.
    fn poll_tick(&mut self, at_ms: u64) -> Effect {
        self.now_ms = at_ms;
        if at_ms < self.next_poll_at_ms {
            return Effect::none();
        }
        self.polls += 1;
        // The schedule advances when the call goes out, and again when the
        // answer lands (with the backoff of however many have failed since).
        self.next_poll_at_ms = at_ms.saturating_add(poll_interval_ms(self.refresh_ms, self.failures));
        // Everything on the sheet that moves is refreshed by one tick: the
        // register always, the run pane's serial while it follows, the
        // machine's uptime while it is on the sheet, and the map's pulse
        // (which needs every run's serial to have anything to measure).
        let mut effect = Effect::poll();
        match &self.pane {
            Pane::Run(id) if self.follow => effect.fetch_run = Some(id.clone()),
            Pane::Machine => effect.poll_machine = true,
            Pane::Map => {
                effect.poll_machine = true;
                effect.fetch_map = self.run_ids();
            }
            _ => {}
        }
        effect
    }

    /// Every run in the register, by id: what the map's pulse is fed from.
    fn run_ids(&self) -> Vec<String> {
        self.runs.iter().map(|run| run.id.clone()).collect()
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

    /// Acts on a key the register binds.
    fn key(&mut self, action: KeyAction) -> Effect {
        match action {
            KeyAction::Register => self.show(Pane::Fleet),
            KeyAction::Machine => self.show(Pane::Machine),
            KeyAction::Map => self.show(Pane::Map),
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
    /// pulse is the delta between this poll and the last.
    fn observe(&mut self, run: &Run) {
        for vm in &run.vms {
            let Some(serial) = vm.serial.as_deref() else {
                continue;
            };
            self.activity
                .observe(&run.id, vm.id, serial.lines().count(), self.now_ms);
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

/// The window title: the register's name and the sheet that is showing.
pub fn title(model: &Model) -> String {
    match &model.pane {
        Pane::Fleet => "AgentPlaza register — fleet".to_string(),
        Pane::Run(id) => format!("AgentPlaza register — {id}"),
        Pane::Spawn => "AgentPlaza register — new run".to_string(),
        Pane::Machine => "AgentPlaza register — machine".to_string(),
        Pane::Map => "AgentPlaza register — fleet map".to_string(),
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

/// The last `limit` lines of a serial capture, and how many lines were left
/// out above them.
pub fn serial_tail(capture: &str, limit: usize) -> (usize, Vec<&str>) {
    let lines: Vec<&str> = capture.lines().collect();
    let skipped = lines.len().saturating_sub(limit);
    (skipped, lines[skipped..].to_vec())
}

/// A duration as the register writes it: seconds, then minutes, then hours.
pub fn duration_label(ms: u64) -> String {
    let seconds = ms / 1_000;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m {:02}s", seconds % 60);
    }
    format!("{}h {:02}m", minutes / 60, minutes % 60)
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{RunState, Vm, VmState};

    fn vm(id: u32, state: VmState, cid: Option<u64>, serial: Option<&str>) -> Vm {
        Vm {
            id,
            guest_cid: cid,
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
        let (skipped, lines) = serial_tail(capture, 2);
        assert_eq!(skipped, 2);
        assert_eq!(lines, vec!["three", "four"]);

        let (skipped, lines) = serial_tail(capture, SERIAL_TAIL_LINES);
        assert_eq!(skipped, 0);
        assert_eq!(lines.len(), 4);

        let (skipped, lines) = serial_tail("", 4);
        assert_eq!((skipped, lines.len()), (0, 0));
    }

    #[test]
    fn a_serial_captures_carriage_returns_do_not_reach_the_lines() {
        let (_, lines) = serial_tail("login: plaza\r\n$ uname\r\nLinux\r\n", 10);
        assert_eq!(lines, vec!["login: plaza", "$ uname", "Linux"]);
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
    fn clock_labels_are_utc() {
        assert_eq!(clock_label(0), "00:00:00 UTC");
        assert_eq!(clock_label(1_760_000_000_000), "08:53:20 UTC");
        assert_eq!(clock_label(86_400_000 + 3_661_000), "01:01:01 UTC");
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
}
