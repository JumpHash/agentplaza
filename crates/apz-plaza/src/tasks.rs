// This crate is safe Rust only: `forbid(unsafe_code)` makes any future
// `unsafe` block a compile error (enforced at the crate root in lib.rs).

//! The task ledger: the plaza's tasks, the states they move through, and the
//! claims a live agent makes on them.
//!
//! The ledger is `core.tasks` (`docs/orchestrator-draft/schema.sql`) restricted
//! to what noded v0 implements, and it moves a task only along the pairs
//! `core.task_transitions` lists — the same lifecycle
//! `docs/formal/TaskLifecycle.tla` models. v0's four moves (`open` → `claimed`,
//! and then `done`, `failed` or `withdrawn`) are a subset of the schema's
//! thirteen-state machine, named for what they mean here:
//!
//! | v0 state | schema state | meaning |
//! |---|---|---|
//! | `open` | `draft` → `ready` | created and claimable (v0 creates tasks ready) |
//! | `claimed` | `assigned` | bound to a live agent; `assignee_id` is set |
//! | `done` | `accepted` | the claiming agent reports the work done |
//! | `failed` | `failed` | the claiming agent reports it failed |
//! | `withdrawn` | `cancelled` | given up before it finished |
//!
//! Every transition that is not one of those is refused with a
//! [`RefusedTransition`], exactly as `core.check_task` refuses a pair that is
//! not in `core.task_transitions`; a task's terminal state is final. A task's
//! `outcome` is the schema's, and the schema's rule that it is set when the
//! task ends is kept: `done` records `accepted`, `failed` records `failed`,
//! `withdrawn` records `cancelled`.
//!
//! What v0 does not do, and says so rather than pretending:
//!
//! * **No verify step.** The schema separates `assigned`/`running` (the worker)
//!   from `verifying`/`accepted` (someone else, 0053). v0 has no verifier, so
//!   `verifier_id` is always null and a claim's word is what finishes a task.
//! * **No bids, dependencies, rework, change sets or tree.** The schema's
//!   `parent_id`/`root_id`/`task_deps`/`change_sets` are a later slice; v0's
//!   tasks are roots with no dependencies.
//! * **Who may finish is recorded, not enforced.** The claim binds the task to
//!   the claiming agent's CID and role; the finish body carries no CID and the
//!   v0 HTTP surface has no authenticated caller, so v0 records the claim and
//!   accepts the finish from anyone who can reach the loopback API. The
//!   identity slice (device keys, agent certificates) is what turns that into
//!   an enforced check.
//! * **No durability.** v0 is in memory, like runs; the schema's tables,
//!   its `updated_at` trigger and its append-only `task_events` are a later
//!   slice. Every transition does publish to the event bus, so the console
//!   sees the ledger live, and [`MAX_TASKS`] bounds what the process holds.
//!
//! # The task's escrow
//!
//! A task's budget is real money in the treasury's journal (see
//! [`crate::treasury`]): a task created with a budget mints it into the task's
//! escrow (a `mint` entry, the mint debited and the escrow credited), and a
//! task that reaches a terminal state (`done`, `failed` or `withdrawn`)
//! refunds what its escrow still holds (a `refund` entry). v0 meters no real
//! spend, so a task that ends refunds its *whole* budget — the treasury view
//! carries that fact in its own words (`spent: 0`, and the note).
//!
//! The two effects are ordered so the ledger is never left inconsistent:
//! `create` mints **before** the task exists (a mint the treasury refuses
//! means no task is created and no id handed out), and `finish`/`withdraw`
//! refund **before** the state is committed (a refund the treasury refuses
//! leaves the task exactly as it was, in the state it had). This adds an
//! effect to four existing transitions but changes no state machine: the
//! states and the allowed pairs are exactly as they were, and the refund a
//! terminal task performs is `docs/formal/Economy.tla`'s `Close` and
//! `TaskLifecycle.tla`'s release, not a transition either model lacks.

use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::events::{Event, EventBus};
use crate::{ConnectedAgent, now_ms};
use crate::treasury::{PostError, Treasury};

/// The most tasks the ledger holds. v0 never removes a task (a task ends
/// `done`, `failed` or `withdrawn` and stays readable), so the cap is
/// absolute: past it the ledger refuses with an honest error rather than
/// dropping the oldest task.
pub const MAX_TASKS: usize = 1024;

/// The most one task text field (the goal, the acceptance criteria, a finish
/// note) may be, in bytes. The HTTP body limit already bounds one request;
/// this bounds what a full ledger of 1024 tasks can hold together.
pub const MAX_TEXT: usize = 4096;

/// The most a role name may be, in bytes.
pub const MAX_ROLE: usize = 64;

/// The states noded v0 gives a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// Created and claimable.
    Open,
    /// Bound to a live agent.
    Claimed,
    /// Ended: the agent reports the work done.
    Done,
    /// Ended: the agent reports the work failed.
    Failed,
    /// Ended: given up before it finished.
    Withdrawn,
}

impl TaskState {
    /// Every state, in the order a task moves through them.
    pub const ALL: [TaskState; 5] = [
        TaskState::Open,
        TaskState::Claimed,
        TaskState::Done,
        TaskState::Failed,
        TaskState::Withdrawn,
    ];

    /// The name the API and the schema's equivalents use.
    pub fn as_str(self) -> &'static str {
        match self {
            TaskState::Open => "open",
            TaskState::Claimed => "claimed",
            TaskState::Done => "done",
            TaskState::Failed => "failed",
            TaskState::Withdrawn => "withdrawn",
        }
    }

    /// Whether `self -> next` is one of the transitions v0 allows: a task is
    /// claimed from `open`, finished from `claimed`, and withdrawn from either
    /// state before it ends. Every other pair is refused.
    pub fn can_transition_to(self, next: TaskState) -> bool {
        matches!(
            (self, next),
            (TaskState::Open, TaskState::Claimed)
                | (TaskState::Claimed, TaskState::Done)
                | (TaskState::Claimed, TaskState::Failed)
                | (TaskState::Open, TaskState::Withdrawn)
                | (TaskState::Claimed, TaskState::Withdrawn)
        )
    }

    /// Whether the task is over: these are the states the schema pairs with a
    /// non-null `outcome`.
    pub fn is_finished(self) -> bool {
        matches!(
            self,
            TaskState::Done | TaskState::Failed | TaskState::Withdrawn
        )
    }

    /// The schema's `outcome` for a task that ends in this state: the state it
    /// ended from (`core.check_task` sets `outcome := OLD.state` when a task
    /// closes), in the schema's outcome enum. `None` while the task lives.
    fn outcome(self) -> Option<&'static str> {
        match self {
            TaskState::Open | TaskState::Claimed => None,
            TaskState::Done => Some("accepted"),
            TaskState::Failed => Some("failed"),
            TaskState::Withdrawn => Some("cancelled"),
        }
    }
}

/// A transition the task state machine does not allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefusedTransition {
    /// The state the task was in.
    pub from: TaskState,
    /// The state the caller asked for.
    pub to: TaskState,
}

impl std::fmt::Display for RefusedTransition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "a task may not go from {} to {}",
            self.from.as_str(),
            self.to.as_str()
        )
    }
}

impl std::error::Error for RefusedTransition {}

/// Why a claim was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimError {
    /// No live agent of this node has that guest CID: the CID is unknown, or
    /// its VM is not running right now. A claim is only ever accepted from an
    /// agent that is connected.
    AgentNotConnected {
        /// The guest CID the caller asked to claim as.
        cid: u64,
    },
    /// The task's state machine does not allow the claim (it is already
    /// claimed, or it has ended).
    Refused(RefusedTransition),
}

impl ClaimError {
    /// The HTTP status this error is reported as. Both refusals are conflicts
    /// with the node's state (no live agent, or a task that is not open), not
    /// malformed requests.
    pub fn status(&self) -> u16 {
        409
    }
}

impl std::fmt::Display for ClaimError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClaimError::AgentNotConnected { cid } => {
                write!(formatter, "no live agent has guest CID {cid}")
            }
            ClaimError::Refused(refused) => refused.fmt(formatter),
        }
    }
}

impl std::error::Error for ClaimError {}

/// Why a task could not be created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateError {
    /// The request is outside the ledger's contract.
    Invalid(String),
    /// The ledger is full.
    Full {
        /// How many tasks the ledger holds.
        max: usize,
    },
    /// The task's budget could not be minted into its escrow, so no task was
    /// created. The journal is full or the accounts cap is reached — the only
    /// ways a well-formed mint is refused.
    Treasury(PostError),
}

impl CreateError {
    /// The HTTP status this error is reported as: a request the ledger cannot
    /// read is a bad request, a ledger at its cap is a conflict with its
    /// state, and a treasury that cannot hold the escrow is one too.
    pub fn status(&self) -> u16 {
        match self {
            CreateError::Invalid(_) => 400,
            CreateError::Full { .. } => 409,
            CreateError::Treasury(error) => error.status(),
        }
    }
}

impl std::fmt::Display for CreateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CreateError::Invalid(reason) => write!(formatter, "{reason}"),
            CreateError::Full { max } => write!(formatter, "the task ledger is full ({max} tasks)"),
            CreateError::Treasury(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for CreateError {}

/// Why a task could not be ended or withdrawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndError {
    /// The state machine refused the transition.
    Refused(RefusedTransition),
    /// The escrow could not be refunded, so the transition was not made: a
    /// terminal task's escrow is always released, or the task does not end.
    Treasury(PostError),
}

impl EndError {
    /// The HTTP status this error is reported as: a refused transition is a
    /// conflict with the task's state, and a treasury refusal is its own.
    pub fn status(&self) -> u16 {
        match self {
            EndError::Refused(_) => 409,
            EndError::Treasury(error) => error.status(),
        }
    }
}

impl std::fmt::Display for EndError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EndError::Refused(refused) => write!(formatter, "{refused}"),
            EndError::Treasury(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for EndError {}

/// The `POST /v1/tasks` body.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateTask {
    /// What the task is for (the schema's `title`), as the caller wrote it.
    pub goal: String,
    /// How the work will be judged complete (the schema's `spec_ref`).
    #[serde(default)]
    pub acceptance: Option<String>,
    /// The task's budget, in credits (the schema's `budget_credits`, whose
    /// default is 0).
    #[serde(default)]
    pub budget: Option<u64>,
    /// The role the task is for, when it names one. v0 records it; it does not
    /// gate a claim on it (the assignment engine that would is a later slice).
    #[serde(default)]
    pub role: Option<String>,
}

/// A create request's fields, checked.
struct Checked {
    goal: String,
    acceptance: Option<String>,
    budget: u64,
    role: Option<String>,
}

impl CreateTask {
    /// Checks the request and returns the fields a task keeps, or says why the
    /// request is outside the ledger's contract.
    fn checked(&self) -> Result<Checked, String> {
        let budget = self.budget.unwrap_or(0);
        // The schema's `budget_credits` is a bigint.
        if budget > i64::MAX as u64 {
            return Err(format!("budget must be at most {}", i64::MAX));
        }
        Ok(Checked {
            goal: required_text("goal", &self.goal, MAX_TEXT)?,
            acceptance: optional_text("acceptance", &self.acceptance, MAX_TEXT)?,
            budget,
            role: optional_text("role", &self.role, MAX_ROLE)?,
        })
    }
}

/// A required text field: present, not blank, and within its bound.
///
/// Shared with [`crate::social`], which holds a board post's and a
/// conversation's text to the same rule — one place where "a text field" is
/// defined, so the board and the ledger cannot drift apart on what an empty or
/// over-long field means.
pub(crate) fn required_text(name: &str, value: &str, max: usize) -> Result<String, String> {
    if value.trim().is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    if value.len() > max {
        return Err(format!("{name} must be at most {max} bytes"));
    }
    Ok(value.to_string())
}

/// An optional text field: absent, or present and held to the same rule
/// ([`required_text`], whose `pub(crate)` visibility this shares).
pub(crate) fn optional_text(name: &str, value: &Option<String>, max: usize) -> Result<Option<String>, String> {
    match value {
        None => Ok(None),
        Some(value) => required_text(name, value, max).map(Some),
    }
}

/// The outcome the claiming agent reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishOutcome {
    /// The work is done.
    Done,
    /// The work failed.
    Failed,
}

impl FinishOutcome {
    /// The state a finished task ends in.
    fn state(self) -> TaskState {
        match self {
            FinishOutcome::Done => TaskState::Done,
            FinishOutcome::Failed => TaskState::Failed,
        }
    }
}

/// The `POST /v1/tasks/{id}/finish` body.
#[derive(Debug, Clone, Deserialize)]
pub struct FinishRequest {
    /// How the task ended: `done` or `failed`.
    pub outcome: FinishOutcome,
    /// What the agent has to say about it, when it says anything.
    #[serde(default)]
    pub note: Option<String>,
}

impl FinishRequest {
    /// Checks the request's own fields, or says why it is outside the
    /// contract.
    pub fn checked(&self) -> Result<(), String> {
        optional_text("note", &self.note, MAX_TEXT).map(|_| ())
    }
}

/// The `POST /v1/tasks/{id}/claim` body.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct ClaimRequest {
    /// The guest CID the claiming agent is connected as.
    pub agent_cid: u64,
}

/// The part of a task its transitions change.
#[derive(Debug)]
struct TaskInner {
    /// The task's state.
    state: TaskState,
    /// The schema's `outcome`, set when the task ends.
    outcome: Option<&'static str>,
    /// The schema's `assignee_id`: the claiming agent's role, or `cid:<n>` for
    /// an agent that has no role (see [`ConnectedAgent::assignee_id`]).
    assignee_id: Option<String>,
    /// The claiming agent's guest CID — v0's own record of who claimed it,
    /// which is what a later identity slice checks a finish against.
    agent_cid: Option<u64>,
    /// The schema's `verifier_id`. v0 has no verification step, so this is
    /// always null; the field exists so the shape is the schema's and the
    /// verify slice has its column.
    verifier_id: Option<String>,
    /// What the agent said when it finished.
    note: Option<String>,
    /// When the task last changed, in milliseconds since the Unix epoch.
    updated_at: u64,
}

/// One task: what the API reports about it, and the state behind the lock.
#[derive(Debug)]
pub struct Task {
    /// The task's id (`t<seq>`).
    id: String,
    /// The schema's `title`.
    goal: String,
    /// The schema's `spec_ref`.
    acceptance: Option<String>,
    /// The schema's `budget_credits`.
    budget: u64,
    /// The role the task was created for, when it named one.
    role: Option<String>,
    /// The treasury entry that minted this task's budget into its escrow, when
    /// it had one: set once, when the task was created, and never changed. The
    /// agora reads it as a call's `escrow_entry`.
    escrow_entry: Option<u64>,
    /// When the task was created, in milliseconds since the Unix epoch.
    created_at: u64,
    /// Where this task's transitions are published.
    events: Arc<EventBus>,
    /// The treasury this task's escrow lives in: its budget was minted there,
    /// and its terminal state refunds what is left.
    treasury: Arc<Treasury>,
    inner: Mutex<TaskInner>,
}

/// A task as the API reports it: the schema's fields and the ones v0 records
/// on them.
///
/// The task's own text (`id`, `goal`, `acceptance`, `role`) is borrowed from
/// the task; everything a transition changes is read out from behind the task's
/// lock while the view is built, so the state fields are owned here.
#[derive(Debug, Serialize)]
pub struct TaskView<'a> {
    /// The task's id (`t<seq>`).
    pub id: &'a str,
    /// What the task is for (the schema's `title`).
    pub goal: &'a str,
    /// How it will be judged complete (the schema's `spec_ref`), or `null`.
    pub acceptance: Option<&'a str>,
    /// The task's budget, in credits (the schema's `budget_credits`).
    pub budget: u64,
    /// The role the task was created for, or `null`.
    pub role: Option<&'a str>,
    /// The task's state.
    pub state: &'static str,
    /// How the task ended, in the schema's outcome enum, or `null` while it
    /// lives.
    pub outcome: Option<&'static str>,
    /// The schema's `assignee_id`: the claiming agent's role, or `cid:<n>`.
    pub assignee_id: Option<String>,
    /// The schema's `verifier_id` (always `null` at v0).
    pub verifier_id: Option<String>,
    /// The claiming agent's guest CID, or `null` while nobody claimed it.
    pub agent_cid: Option<u64>,
    /// What the agent said when it finished, or `null`.
    pub note: Option<String>,
    /// When the task was created, in milliseconds since the Unix epoch.
    pub created_at: u64,
    /// When the task last changed, in milliseconds since the Unix epoch.
    pub updated_at: u64,
}

/// The list endpoint's body: `{"tasks": [...]}`.
#[derive(Debug, Serialize)]
pub struct TasksView<'a> {
    /// Every task, oldest first.
    pub tasks: Vec<TaskView<'a>>,
}

impl Task {
    /// The task's id (`t<seq>`).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The role the task was created for, when it named one — what a call's
    /// `capability` is derived from.
    pub fn role(&self) -> Option<&str> {
        self.role.as_deref()
    }

    /// The task's budget, in credits — what a call's `max_price` is derived
    /// from.
    pub fn budget(&self) -> u64 {
        self.budget
    }

    /// How the work will be judged complete (the schema's `spec_ref`), when
    /// the task named one — what a call's `spec_ref` is derived from.
    pub fn acceptance(&self) -> Option<&str> {
        self.acceptance.as_deref()
    }

    /// What the task is for (the schema's `title`) — what a hiring note's body
    /// is composed from (see [`crate::social`]).
    pub fn goal(&self) -> &str {
        &self.goal
    }

    /// When the task was created, in milliseconds since the Unix epoch.
    pub fn created_at(&self) -> u64 {
        self.created_at
    }

    /// The task's state name (`open`, `claimed`, ...).
    pub fn state(&self) -> &'static str {
        self.lock().state.as_str()
    }

    /// Whether the task is still `open` — the one state a claim, a bid or an
    /// award can move it out of.
    pub fn is_open(&self) -> bool {
        self.lock().state == TaskState::Open
    }

    /// The treasury entry that minted this task's budget into its escrow, when
    /// it had a budget. The agora records it as a call's `escrow_entry`: at v0
    /// a task's escrow is funded by its own mint, not by a parent task's
    /// subcontract.
    pub fn escrow_entry(&self) -> Option<u64> {
        self.escrow_entry
    }

    /// The task as the API reports it.
    pub fn view(&self) -> TaskView<'_> {
        let inner = self.lock();
        TaskView {
            id: &self.id,
            goal: &self.goal,
            acceptance: self.acceptance.as_deref(),
            budget: self.budget,
            role: self.role.as_deref(),
            state: inner.state.as_str(),
            outcome: inner.outcome,
            assignee_id: inner.assignee_id.clone(),
            verifier_id: inner.verifier_id.clone(),
            agent_cid: inner.agent_cid,
            note: inner.note.clone(),
            created_at: self.created_at,
            updated_at: inner.updated_at,
        }
    }

    /// Binds the task to `agent`, whose liveness this task's caller has already
    /// established against the node's register.
    ///
    /// Only an `open` task can be claimed: a second claim on a claimed task is
    /// refused by the state machine, like every other transition. A refused
    /// claim changes nothing.
    pub fn claim(&self, agent: &ConnectedAgent) -> Result<(), RefusedTransition> {
        let mut inner = self.lock();
        let from = inner.state;
        if !from.can_transition_to(TaskState::Claimed) {
            return Err(RefusedTransition {
                from,
                to: TaskState::Claimed,
            });
        }
        let assignee = agent.assignee_id();
        inner.state = TaskState::Claimed;
        inner.assignee_id = Some(assignee.clone());
        inner.agent_cid = Some(agent.cid);
        inner.updated_at = now_ms();
        self.events.publish(Event::Task {
            id: self.id.clone(),
            state: TaskState::Claimed.as_str(),
            assignee: Some(assignee),
        });
        Ok(())
    }

    /// Ends the task as `outcome`, with what the agent has to say about it.
    ///
    /// Only a claimed task can be finished. The claiming agent's CID is
    /// recorded at claim time; nothing in a finish request identifies the
    /// caller, so v0 records who claimed the task rather than proving who
    /// finished it (see the module note on the identity slice).
    ///
    /// Ending a task releases its escrow: what the task's escrow still holds
    /// goes back to the mint (a `refund` entry). The refund is posted **before**
    /// the state is committed, so a treasury that refuses it leaves the task in
    /// the state it had — a terminal task is never left holding its budget.
    pub fn finish(
        &self,
        outcome: FinishOutcome,
        note: Option<String>,
    ) -> Result<(), EndError> {
        let to = outcome.state();
        let mut inner = self.lock();
        let from = inner.state;
        if !from.can_transition_to(to) {
            return Err(EndError::Refused(RefusedTransition { from, to }));
        }
        self.release_escrow()?;
        inner.state = to;
        inner.outcome = to.outcome();
        inner.note = note;
        inner.updated_at = now_ms();
        let assignee = inner.assignee_id.clone();
        self.events.publish(Event::Task {
            id: self.id.clone(),
            state: to.as_str(),
            assignee,
        });
        Ok(())
    }

    /// Ends the task as `withdrawn`, from `open` or `claimed`.
    ///
    /// A task that has already ended is final, so withdrawing one is refused.
    /// Like a finish, a withdrawal releases the escrow (a `refund` entry) and
    /// only commits the state if the treasury accepts it.
    pub fn withdraw(&self) -> Result<(), EndError> {
        let to = TaskState::Withdrawn;
        let mut inner = self.lock();
        let from = inner.state;
        if !from.can_transition_to(to) {
            return Err(EndError::Refused(RefusedTransition { from, to }));
        }
        self.release_escrow()?;
        inner.state = to;
        inner.outcome = to.outcome();
        inner.updated_at = now_ms();
        let assignee = inner.assignee_id.clone();
        self.events.publish(Event::Task {
            id: self.id.clone(),
            state: to.as_str(),
            assignee,
        });
        Ok(())
    }

    /// Refunds whatever this task's escrow holds, if anything. A task with no
    /// budget, or one whose escrow is already empty, posts nothing.
    fn release_escrow(&self) -> Result<(), EndError> {
        self.treasury
            .refund_task_to_budget(&self.id)
            .map(|_| ())
            .map_err(EndError::Treasury)
    }

    fn lock(&self) -> MutexGuard<'_, TaskInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The node's tasks, in creation order, the bus their transitions are
/// published to, and the treasury their budgets are escrowed in.
pub struct TaskLedger {
    events: Arc<EventBus>,
    treasury: Arc<Treasury>,
    inner: Mutex<LedgerInner>,
}

struct LedgerInner {
    next_seq: u64,
    tasks: Vec<Arc<Task>>,
}

impl TaskLedger {
    /// A ledger with no tasks that publishes every transition to `events`, and
    /// whose tasks' budgets are escrowed in a treasury that publishes to the
    /// same bus.
    pub fn new(events: Arc<EventBus>) -> Self {
        Self {
            treasury: Arc::new(Treasury::new(Arc::clone(&events))),
            events,
            inner: Mutex::new(LedgerInner {
                next_seq: 0,
                tasks: Vec::new(),
            }),
        }
    }

    /// The treasury the ledger's tasks are escrowed in (the daemon's treasury
    /// view and journal read it).
    pub fn treasury(&self) -> &Arc<Treasury> {
        &self.treasury
    }

    /// Creates the next task in `open`, where a v0 task begins.
    ///
    /// The request is checked and the cap applied before an id is handed out,
    /// so a refused create leaves the ledger exactly as it was. A positive
    /// budget is funded from the plaza's budget into the task's escrow before the task
    /// is registered:
    /// a funding the treasury refuses (a short plaza budget) fails the create and
    /// creates no task, so a task never exists without its escrow.
    pub fn create(&self, request: &CreateTask) -> Result<Arc<Task>, CreateError> {
        let checked = request.checked().map_err(CreateError::Invalid)?;
        let mut inner = self.lock();
        if inner.tasks.len() >= MAX_TASKS {
            return Err(CreateError::Full { max: MAX_TASKS });
        }
        // The id is reserved here but `next_seq` only advances once the budget
        // is minted, so a refused mint hands out no id.
        let id = format!("t{}", inner.next_seq + 1);
        // The entry the budget was minted by, when there was one: the task
        // keeps it, and the agora records it as a call's `escrow_entry`.
        let mut entry = None;
        if checked.budget > 0 {
            // `checked` bounds the budget to `i64::MAX`.
            let amount = i64::try_from(checked.budget).unwrap_or(i64::MAX);
            entry = Some(
                self.treasury
                    .fund_task(&id, amount)
                    .map_err(CreateError::Treasury)?,
            );
        }
        inner.next_seq += 1;
        let now = now_ms();
        let task = Arc::new(Task {
            id,
            goal: checked.goal,
            acceptance: checked.acceptance,
            budget: checked.budget,
            role: checked.role,
            escrow_entry: entry,
            created_at: now,
            events: Arc::clone(&self.events),
            treasury: Arc::clone(&self.treasury),
            inner: Mutex::new(TaskInner {
                state: TaskState::Open,
                outcome: None,
                assignee_id: None,
                agent_cid: None,
                verifier_id: None,
                note: None,
                updated_at: now,
            }),
        });
        inner.tasks.push(Arc::clone(&task));
        // The task is registered under the ledger's lock, so a subscriber sees
        // it appear exactly once and in order.
        self.events.publish(Event::Task {
            id: task.id.clone(),
            state: TaskState::Open.as_str(),
            assignee: None,
        });
        Ok(task)
    }

    /// The task with that id, if there is one.
    pub fn find(&self, id: &str) -> Option<Arc<Task>> {
        self.lock().tasks.iter().find(|task| task.id == id).cloned()
    }

    /// Every task, oldest first.
    pub fn list(&self) -> Vec<Arc<Task>> {
        self.lock().tasks.clone()
    }

    fn lock(&self) -> MutexGuard<'_, LedgerInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::time::Duration;

    /// A connected agent with a role, as the register would report one.
    fn agent(cid: u64, role: &str) -> ConnectedAgent {
        ConnectedAgent {
            cid,
            role: Some(role.to_string()),
            run: "r1".to_string(),
            vm: 0,
        }
    }

    fn create(ledger: &TaskLedger, goal: &str) -> Arc<Task> {
        ledger
            .create(&CreateTask {
                goal: goal.to_string(),
                acceptance: None,
                budget: None,
                role: None,
            })
            .expect("a task inside the contract")
    }

    /// A task created with a budget, which the ledger mints into its escrow.
    fn budgeted(ledger: &TaskLedger, goal: &str, budget: u64) -> Arc<Task> {
        ledger
            .create(&CreateTask {
                goal: goal.to_string(),
                acceptance: None,
                budget: Some(budget),
                role: None,
            })
            .expect("a budgeted task inside the contract")
    }


    /// `core.task_transitions`, restricted to the states noded v0 implements.
    const ALLOWED: [(TaskState, TaskState); 5] = [
        (TaskState::Open, TaskState::Claimed),
        (TaskState::Claimed, TaskState::Done),
        (TaskState::Claimed, TaskState::Failed),
        (TaskState::Open, TaskState::Withdrawn),
        (TaskState::Claimed, TaskState::Withdrawn),
    ];

    #[test]
    fn the_transition_matrix_matches_the_schema() {
        for from in TaskState::ALL {
            for to in TaskState::ALL {
                assert_eq!(
                    from.can_transition_to(to),
                    ALLOWED.contains(&(from, to)),
                    "{} -> {}",
                    from.as_str(),
                    to.as_str()
                );
            }
        }
    }

    #[test]
    fn the_state_names_and_outcomes_are_the_schemas() {
        assert_eq!(TaskState::Open.as_str(), "open");
        assert_eq!(TaskState::Claimed.as_str(), "claimed");
        assert_eq!(TaskState::Done.as_str(), "done");
        assert_eq!(TaskState::Failed.as_str(), "failed");
        assert_eq!(TaskState::Withdrawn.as_str(), "withdrawn");
        assert!(!TaskState::Open.is_finished());
        assert!(!TaskState::Claimed.is_finished());
        assert!(TaskState::Done.is_finished());
        assert!(TaskState::Failed.is_finished());
        assert!(TaskState::Withdrawn.is_finished());
        // The schema's outcome enum, and nothing else.
        assert_eq!(TaskState::Open.outcome(), None);
        assert_eq!(TaskState::Claimed.outcome(), None);
        assert_eq!(TaskState::Done.outcome(), Some("accepted"));
        assert_eq!(TaskState::Failed.outcome(), Some("failed"));
        assert_eq!(TaskState::Withdrawn.outcome(), Some("cancelled"));
    }

    #[test]
    fn a_refused_transition_leaves_the_task_alone() {
        let ledger = TaskLedger::new(EventBus::new());
        let task = create(&ledger, "ship the ledger");
        task.claim(&agent(100, "builder")).expect("open -> claimed");
        let claimed = task.view();
        let updated_at = claimed.updated_at;

        let refused = task
            .claim(&agent(101, "reviewer"))
            .expect_err("a claimed task cannot be claimed again");
        assert_eq!(
            refused,
            RefusedTransition {
                from: TaskState::Claimed,
                to: TaskState::Claimed
            }
        );
        assert_eq!(
            refused.to_string(),
            "a task may not go from claimed to claimed"
        );
        let view = task.view();
        assert_eq!(view.state, "claimed");
        assert_eq!(view.assignee_id.as_deref(), Some("builder"));
        assert_eq!(view.agent_cid, Some(100));
        assert_eq!(view.updated_at, updated_at);
    }

    #[test]
    fn a_claim_binds_the_task_to_the_agent() {
        let ledger = TaskLedger::new(EventBus::new());
        let task = create(&ledger, "ship the ledger");

        assert_eq!(task.view().state, "open");
        assert_eq!(task.view().assignee_id, None);
        assert_eq!(task.view().agent_cid, None);

        let builder = ConnectedAgent {
            cid: 100,
            role: Some("builder".to_string()),
            run: "r1".to_string(),
            vm: 2,
        };
        task.claim(&builder).expect("open -> claimed");
        let view = task.view();
        assert_eq!(view.state, "claimed");
        // The schema's `assignee_id` is the agent's role at v0.
        assert_eq!(view.assignee_id.as_deref(), Some("builder"));
        assert_eq!(view.agent_cid, Some(100));
        assert_eq!(view.outcome, None);

        // An agent with no role is named by its CID.
        let other = create(&ledger, "ship it too");
        other
            .claim(&ConnectedAgent {
                cid: 7,
                role: None,
                run: "r1".to_string(),
                vm: 0,
            })
            .expect("open -> claimed");
        assert_eq!(other.view().assignee_id.as_deref(), Some("cid:7"));
        assert_eq!(other.view().agent_cid, Some(7));
    }

    #[test]
    fn a_finish_only_leaves_a_claimed_task() {
        let ledger = TaskLedger::new(EventBus::new());

        // An open task has no worker to finish it.
        let open = create(&ledger, "not started");
        let refused = open
            .finish(FinishOutcome::Done, None)
            .expect_err("open -> done is refused");
        assert_eq!(
            refused.to_string(),
            "a task may not go from open to done"
        );
        assert_eq!(open.view().state, "open");
        assert_eq!(open.view().outcome, None);

        // A claimed task finishes as done or failed, once.
        let done = create(&ledger, "done");
        done.claim(&agent(100, "builder")).expect("claim");
        done.finish(FinishOutcome::Done, Some("ran the suite".to_string()))
            .expect("claimed -> done");
        let view = done.view();
        assert_eq!(view.state, "done");
        assert_eq!(view.outcome, Some("accepted"));
        assert_eq!(view.note.as_deref(), Some("ran the suite"));
        assert_eq!(view.assignee_id.as_deref(), Some("builder"));
        let refused = done
            .finish(FinishOutcome::Failed, None)
            .expect_err("done is final");
        assert_eq!(
            refused.to_string(),
            "a task may not go from done to failed"
        );

        let failed = create(&ledger, "failed");
        failed.claim(&agent(100, "builder")).expect("claim");
        failed
            .finish(FinishOutcome::Failed, None)
            .expect("claimed -> failed");
        assert_eq!(failed.view().state, "failed");
        assert_eq!(failed.view().outcome, Some("failed"));
        assert_eq!(failed.view().note, None);
    }

    #[test]
    fn a_withdraw_ends_an_open_or_claimed_task() {
        let ledger = TaskLedger::new(EventBus::new());

        let open = create(&ledger, "no longer wanted");
        open.withdraw().expect("open -> withdrawn");
        assert_eq!(open.view().state, "withdrawn");
        assert_eq!(open.view().outcome, Some("cancelled"));
        assert_eq!(open.view().assignee_id, None);

        let claimed = create(&ledger, "stopped mid-flight");
        claimed.claim(&agent(100, "builder")).expect("claim");
        claimed.withdraw().expect("claimed -> withdrawn");
        assert_eq!(claimed.view().state, "withdrawn");
        assert_eq!(claimed.view().outcome, Some("cancelled"));
        // The claim is not erased by the withdrawal.
        assert_eq!(claimed.view().assignee_id.as_deref(), Some("builder"));
        assert_eq!(claimed.view().agent_cid, Some(100));

        let refused = claimed
            .withdraw()
            .expect_err("a withdrawn task is final");
        assert_eq!(
            refused.to_string(),
            "a task may not go from withdrawn to withdrawn"
        );
    }

    #[test]
    fn the_ledger_is_capped_and_refuses_honestly() {
        let ledger = TaskLedger::new(EventBus::new());
        let ids: HashSet<String> = (0..MAX_TASKS)
            .map(|index| create(&ledger, &format!("task {index}")).id().to_string())
            .collect();
        assert_eq!(ids.len(), MAX_TASKS, "every id is its own");
        assert_eq!(ledger.list().len(), MAX_TASKS);

        let refused = ledger
            .create(&CreateTask {
                goal: "one too many".to_string(),
                acceptance: None,
                budget: None,
                role: None,
            })
            .expect_err("the ledger is full");
        assert_eq!(refused, CreateError::Full { max: MAX_TASKS });
        assert_eq!(
            refused.to_string(),
            "the task ledger is full (1024 tasks)"
        );
        assert_eq!(refused.status(), 409);
        assert_eq!(
            ledger.list().len(),
            MAX_TASKS,
            "a refused create adds nothing"
        );

        // v0 never removes a task, so withdrawing one does not free a slot -
        // but a fresh create still gets an id no other task has.
        ledger
            .find("t1")
            .expect("the first task")
            .withdraw()
            .expect("open -> withdrawn");
        let refused_again = ledger
            .create(&CreateTask {
                goal: "still too many".to_string(),
                acceptance: None,
                budget: None,
                role: None,
            })
            .expect_err("the withdrawn task still counts");
        assert_eq!(refused_again, CreateError::Full { max: MAX_TASKS });
    }

    #[test]
    fn a_request_outside_the_contract_creates_nothing() {
        let ledger = TaskLedger::new(EventBus::new());
        let request = |goal: &str, acceptance: Option<&str>, budget: Option<u64>, role: Option<&str>| {
            CreateTask {
                goal: goal.to_string(),
                acceptance: acceptance.map(String::from),
                budget,
                role: role.map(String::from),
            }
        };

        for bad in [
            request("", None, None, None),
            request("   ", None, None, None),
            request(&"g".repeat(MAX_TEXT + 1), None, None, None),
            request("g", Some(""), None, None),
            request("g", Some(&"a".repeat(MAX_TEXT + 1)), None, None),
            request("g", None, None, Some("")),
            request("g", None, None, Some(&"r".repeat(MAX_ROLE + 1))),
            request("g", None, Some(u64::MAX), None),
        ] {
            let refused = ledger.create(&bad).expect_err("outside the contract");
            assert_eq!(refused.status(), 400, "{bad:?}");
            assert!(
                matches!(refused, CreateError::Invalid(_)),
                "{bad:?}: {refused}"
            );
        }
        assert!(ledger.list().is_empty(), "no refused request created one");

        ledger.treasury().mint_plaza_budget(i64::MAX).expect("the plaza covers the largest budget");
        // The edges are inside the contract.
        let edges = [
            request("g", None, None, None),
            request("g", Some("a"), Some(i64::MAX as u64), Some("builder")),
            request(&"g".repeat(MAX_TEXT), Some(&"a".repeat(MAX_TEXT)), Some(0), Some(&"r".repeat(MAX_ROLE))),
        ];
        let ids: Vec<String> = edges
            .iter()
            .map(|edge| ledger.create(edge).expect("inside the contract").id().to_string())
            .collect();
        // The refused requests handed out no ids, so the first task is t1.
        assert_eq!(ids, vec!["t1", "t2", "t3"]);

        // The optional fields default to the schema's.
        let defaulted = ledger.create(&request("just a goal", None, None, None)).expect("a task");
        let view = defaulted.view();
        assert_eq!(view.goal, "just a goal");
        assert_eq!(view.acceptance, None);
        assert_eq!(view.budget, 0);
        assert_eq!(view.role, None);
        assert_eq!(view.verifier_id, None);
        assert_eq!(view.note, None);
        assert_eq!(view.created_at, view.updated_at);
    }

    #[test]
    fn the_view_carries_the_schemas_fields() {
        let ledger = TaskLedger::new(EventBus::new());
        let task = ledger
            .create(&CreateTask {
                goal: "ship the ledger".to_string(),
                acceptance: Some("the suite is green".to_string()),
                budget: Some(5),
                role: Some("builder".to_string()),
            })
            .expect("a task");
        task.claim(&agent(100, "builder")).expect("claim");
        task.finish(FinishOutcome::Done, Some("done".to_string()))
            .expect("finish");

        let view = serde_json::to_value(task.view()).expect("the view serializes");
        let created_at = view["created_at"].as_u64().expect("created_at");
        assert!(created_at > 0);
        assert_eq!(view["id"], "t1");
        assert_eq!(view["goal"], "ship the ledger");
        assert_eq!(view["acceptance"], "the suite is green");
        assert_eq!(view["budget"], 5);
        assert_eq!(view["role"], "builder");
        assert_eq!(view["state"], "done");
        assert_eq!(view["outcome"], "accepted");
        assert_eq!(view["assignee_id"], "builder");
        assert_eq!(view["verifier_id"], serde_json::Value::Null);
        assert_eq!(view["agent_cid"], 100);
        assert_eq!(view["note"], "done");
        assert!(view["updated_at"].as_u64().expect("updated_at") >= created_at);

        let list = serde_json::to_value(TasksView {
            tasks: ledger
                .list()
                .iter()
                .map(|task| task.view())
                .collect(),
        })
        .expect("the list serializes");
        assert_eq!(list["tasks"].as_array().expect("an array").len(), 1);
        assert_eq!(list["tasks"][0], view);
    }

    #[test]
    fn every_transition_publishes_its_task_event() {
        let events = EventBus::new();
        let receiver = events.subscribe();
        let ledger = TaskLedger::new(Arc::clone(&events));

        let task = create(&ledger, "a task");
        task.claim(&agent(100, "builder")).expect("claim");
        task.finish(FinishOutcome::Done, None).expect("finish");

        let mut frames = Vec::new();
        while let Ok(event) = receiver.recv_timeout(Duration::from_secs(1)) {
            frames.push(event.frame());
        }
        assert_eq!(
            frames,
            vec![
                "event: treasury\ndata: {\"entry\":1,\"kind\":\"mint\",\"amount\":100000,\"task\":null}\n\n",
                "event: task\ndata: {\"id\":\"t1\",\"state\":\"open\",\"assignee\":null}\n\n",
                "event: task\ndata: {\"id\":\"t1\",\"state\":\"claimed\",\"assignee\":\"builder\"}\n\n",
                "event: task\ndata: {\"id\":\"t1\",\"state\":\"done\",\"assignee\":\"builder\"}\n\n",
            ]
        );

        // A withdrawal carries the claim it had, if any.
        let unclaimed = create(&ledger, "unwanted");
        unclaimed.withdraw().expect("withdraw");
        assert_eq!(
            receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("the open event")
                .frame(),
            "event: task\ndata: {\"id\":\"t2\",\"state\":\"open\",\"assignee\":null}\n\n"
        );
        assert_eq!(
            receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("the withdrawn event")
                .frame(),
            "event: task\ndata: {\"id\":\"t2\",\"state\":\"withdrawn\",\"assignee\":null}\n\n"
        );
    }

    #[test]
    fn a_budgeted_task_mints_its_escrow_and_a_finish_refunds_it() {
        let ledger = TaskLedger::new(EventBus::new());
        let task = budgeted(&ledger, "paid work", 500);

        // Creating it funded the budget from the plaza into its escrow, and only that.
        assert_eq!(ledger.treasury().escrow_balance("t1"), 500);
        assert_eq!(
            ledger.treasury().plaza_budget_balance(),
            i128::from(crate::treasury::PLAZA_BUDGET) - 500
        );
        assert_eq!(ledger.treasury().view().totals.entries, 2);
        assert_eq!(ledger.treasury().view().spent, 0);

        task.claim(&agent(100, "builder")).expect("claim");
        task.finish(FinishOutcome::Done, Some("done".to_string()))
            .expect("finish");

        // v0 meters no real spend, so finishing refunded the whole escrow to the plaza.
        assert_eq!(ledger.treasury().escrow_balance("t1"), 0);
        assert_eq!(ledger.treasury().view().totals.entries, 3);
        assert_eq!(
            ledger.treasury().plaza_budget_balance(),
            i128::from(crate::treasury::PLAZA_BUDGET)
        );
        // The journal balances as a whole.
        assert_eq!(ledger.treasury().view().totals.balance, 0);
        assert!(ledger.treasury().verify_journal().is_ok());
    }
    #[test]
    fn every_terminal_state_refunds_the_whole_escrow() {
        let ledger = TaskLedger::new(EventBus::new());

        let failed = budgeted(&ledger, "failed", 7);
        failed.claim(&agent(100, "builder")).expect("claim");
        failed
            .finish(FinishOutcome::Failed, None)
            .expect("claimed -> failed");
        assert_eq!(ledger.treasury().escrow_balance("t1"), 0);

        let withdrawn = budgeted(&ledger, "withdrawn", 9);
        assert_eq!(ledger.treasury().escrow_balance("t2"), 9);
        withdrawn.withdraw().expect("open -> withdrawn");
        assert_eq!(ledger.treasury().escrow_balance("t2"), 0);

        let claimed = budgeted(&ledger, "withdrawn mid-flight", 3);
        claimed.claim(&agent(100, "builder")).expect("claim");
        claimed.withdraw().expect("claimed -> withdrawn");
        assert_eq!(ledger.treasury().escrow_balance("t3"), 0);

        // The plaza's mint, three fundings and three refunds, and nothing left in any escrow.
        assert_eq!(ledger.treasury().view().totals.entries, 7);
        assert_eq!(
            ledger.treasury().plaza_budget_balance(),
            i128::from(crate::treasury::PLAZA_BUDGET)
        );
        for task in ledger.list() {
            assert_eq!(ledger.treasury().escrow_balance(task.id()), 0);
        }
    }

    #[test]
    fn an_unbudgeted_task_posts_nothing() {
        let ledger = TaskLedger::new(EventBus::new());
        let task = create(&ledger, "free work");
        // No budget, no escrow: the treasury holds only the plaza's mint.
        assert_eq!(ledger.treasury().view().totals.entries, 1);
        assert_eq!(ledger.treasury().accounts().len(), 2);
        assert_eq!(ledger.treasury().escrow_balance("t1"), 0);

        task.claim(&agent(100, "builder")).expect("claim");
        task.finish(FinishOutcome::Done, None).expect("finish");
        // Ending it refunds nothing, because there is nothing to refund.
        assert_eq!(ledger.treasury().view().totals.entries, 1);
        assert_eq!(ledger.treasury().accounts().len(), 2);
    }

    #[test]
    fn a_refused_transition_refunds_nothing() {
        let ledger = TaskLedger::new(EventBus::new());
        let task = budgeted(&ledger, "not started", 50);

        let refused = task
            .finish(FinishOutcome::Done, None)
            .expect_err("open -> done is refused");
        assert_eq!(refused.to_string(), "a task may not go from open to done");
        assert_eq!(refused.status(), 409);
        // The refused transition left the escrow exactly where it was.
        assert_eq!(ledger.treasury().escrow_balance("t1"), 50);
        assert_eq!(ledger.treasury().view().totals.entries, 2);

        // A second withdraw is refused, and refunds nothing a second time.
        task.withdraw().expect("open -> withdrawn");
        assert_eq!(ledger.treasury().view().totals.entries, 3);
        let refused = task.withdraw().expect_err("withdrawn is final");
        assert_eq!(
            refused.to_string(),
            "a task may not go from withdrawn to withdrawn"
        );
        assert_eq!(ledger.treasury().view().totals.entries, 3);
    }

    #[test]
    fn a_task_the_treasury_cannot_fund_is_not_created() {
        let ledger = TaskLedger::new(EventBus::new());
        // Fill the accounts: any fresh escrow needs one more than the cap.
        for index in 0..crate::treasury::MAX_ACCOUNTS - 2 {
            ledger
                .treasury()
                .open_account(crate::treasury::AccountKind::TownBudget, &format!("town{index}"), 0)
                .expect("an account under the cap");
        }
        let refused = ledger
            .create(&CreateTask {
                goal: "cannot be funded".to_string(),
                acceptance: None,
                budget: Some(5),
                role: None,
            })
            .expect_err("the treasury cannot open the escrow");
        assert!(
            matches!(
                refused,
                CreateError::Treasury(PostError::AccountsFull { .. })
            ),
            "{refused:?}"
        );
        assert_eq!(refused.status(), 409);
        // No task was created, no entry posted, and the id was not consumed:
        // an unbudgeted task (which needs no escrow) still gets `t1`.
        assert!(ledger.list().is_empty());
        assert_eq!(ledger.treasury().view().totals.entries, 1);
        assert_eq!(create(&ledger, "free").id(), "t1");
    }
}
