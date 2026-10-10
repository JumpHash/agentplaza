//! The dashboard's headless core: the panes, the state, the reducer and the
//! poll arithmetic.
//!
//! Nothing here knows about iced, so the whole model — parsing outcomes, the
//! reducer, validation and the poll schedule — is unit-tested without a
//! display or a window. `main` turns an [`Effect`] into an iced [`Task`] and
//! nothing more.

use crate::api::{ApiError, CreateRun, Endpoint, Run, RunsPage};
use crate::form::{Field, SpawnForm};

/// How often a healthy daemon is polled.
pub const POLL_INTERVAL_MS: u64 = 2_000;

/// The longest the poll backs off to while calls keep failing.
pub const MAX_POLL_INTERVAL_MS: u64 = 30_000;

/// How long a good answer stays "live" before the register calls it stale.
pub const STALE_AFTER_MS: u64 = 5_000;

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
    /// Fetch `GET /api/v0/runs/{id}`.
    pub fetch_run: Option<String>,
    /// `DELETE /api/v0/runs/{id}`.
    pub kill: Option<String>,
    /// `POST /api/v0/runs` with this body.
    pub spawn: Option<CreateRun>,
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
            | Message::Killed { at_ms, .. }
            | Message::Spawned { at_ms, .. } => Some(*at_ms),
            Message::Show(_)
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
    /// Whether the run pane follows the serial as it grows.
    pub follow: bool,
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
            follow: true,
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

    /// How long a run has been up: created to ended, or created to now.
    pub fn run_elapsed_ms(&self, run: &Run) -> u64 {
        run.ended_at
            .unwrap_or(self.now_ms)
            .saturating_sub(run.created_at)
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
                        self.succeeded(at_ms);
                    }
                    Err(error) => self.failed(at_ms, &error),
                }
                Effect::none()
            }
            Message::Run { at_ms, id, result } => self.run_answer(at_ms, &id, result),
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
        self.next_poll_at_ms = at_ms.saturating_add(poll_interval_ms(self.failures));
        match self.pane.clone() {
            Pane::Run(id) if self.follow => Effect::refresh(id),
            _ => Effect::poll(),
        }
    }

    /// A single-run answer, which also tells the register how to read the run.
    fn run_answer(&mut self, at_ms: u64, id: &str, result: Result<Run, ApiError>) -> Effect {
        match result {
            Ok(run) => {
                self.succeeded(at_ms);
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
        self.next_poll_at_ms = at_ms.saturating_add(POLL_INTERVAL_MS);
    }

    /// Records a call that failed, and backs the poll off.
    fn failed(&mut self, at_ms: u64, error: &ApiError) {
        self.failures = self.failures.saturating_add(1);
        self.last_error = Some(error.to_string());
        self.next_poll_at_ms = at_ms.saturating_add(poll_interval_ms(self.failures));
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
/// row: the healthy 2 s, doubling to a 30 s ceiling.
pub fn poll_interval_ms(failures: u32) -> u64 {
    let shift = failures.min(4);
    (POLL_INTERVAL_MS << shift).min(MAX_POLL_INTERVAL_MS)
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
        assert_eq!(poll_interval_ms(0), 2_000);
        assert_eq!(poll_interval_ms(1), 4_000);
        assert_eq!(poll_interval_ms(2), 8_000);
        assert_eq!(poll_interval_ms(3), 16_000);
        assert_eq!(poll_interval_ms(4), 30_000, "capped");
        assert_eq!(poll_interval_ms(40), 30_000);

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
        assert_eq!(model.next_poll_at_ms, 9_000 + POLL_INTERVAL_MS);
        assert_eq!(model.health(), Health::Live);
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

    #[test]
    fn every_health_has_a_label() {
        for health in [Health::Unknown, Health::Live, Health::Stale, Health::Down] {
            assert!(!health.label().is_empty());
        }
    }
}
