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
//! | `open` | `draft` → `ready` | created; claimable once every task it runs after is done (a task with no links at once) |
//! | `claimed` | `assigned` | bound to a live agent; `assignee_id` is set |
//! | `awaiting_review` | `verifying` | the claiming agent reports the work done; it waits for a reviewer's verdict |
//! | `done` | `accepted` | the work is accepted: a reviewer passes it, the owner approves it, or a claim is finished as done |
//! | `failed` | `failed` | the claiming agent reports it failed, or a verdict fails it |
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
//! * **A review, but no verifier record.** The schema separates
//!   `assigned`/`running` (the worker) from `verifying`/`accepted` (someone
//!   else, 0053). A claim the agent reports done moves to `awaiting_review`
//!   and waits there for a reviewer's verdict; the scheduler holds that review,
//!   so `verifier_id` is always null.
//! * **Links, but no tree.** A task may run after other tasks (`after`, the
//!   schema's `task_deps`): it is *ready*, and only then claimable, once every
//!   one of them is `done` — `TaskLifecycle.tla`'s `DepsAcceptedBeforeWork`.
//!   Links stay inside a project (the schema keeps them inside a task tree),
//!   number at most [`MAX_AFTER`] per task, and never close a cycle. A link that
//!   fails or is withdrawn does not cascade: the task stays `open`, names the
//!   link in `blocked_by`, and waits to be relinked or withdrawn. No bids,
//!   rework, change sets or tree: the schema's `parent_id`/`root_id`/
//!   `change_sets` are a later slice, and v0's tasks are roots.
//! * **Who may finish is recorded, not enforced.** The claim binds the task to
//!   the claiming agent's CID and role; the finish body carries no CID and the
//!   v0 HTTP surface has no authenticated caller, so v0 records the claim and
//!   accepts the finish from anyone who can reach the loopback API. The
//!   identity slice (device keys, agent certificates) is what turns that into
//!   an enforced check.
//! * **Persistence is apid's job.** The ledger is held in memory and apid
//!   snapshots it (`snapshot` / `restore`) into Postgres when it has one
//!   (without, the ledger lasts as long as the process); a task's links and
//!   project are in its record, and a record from before them loads with none. The schema's tables (`task_deps` among them), its
//!   `updated_at` trigger and its append-only `task_events` are a later slice.
//!   Every transition does publish to the event bus, so the console sees the
//!   ledger live, and [`MAX_TASKS`] bounds what the process holds.
//!
//! # The task's escrow
//!
//! A task's budget is real money in the treasury's journal (see
//! [`crate::treasury`]): a task created with a budget mints it into the task's
//! escrow (a `mint` entry, the mint debited and the escrow credited), and a
//! task that reaches a terminal state (`done`, `failed` or `withdrawn`)
//! refunds what its escrow still holds (a `refund` entry). Model calls are
//! metered as real spend (see [`crate::treasury`]), so what a task ends with is
//! its budget less that spend, and the treasury view's `spent` totals it. A task in
//! `awaiting_review` is not terminal: its escrow stays held until a verdict
//! finishes it, so the reviewed worker is paid out of that escrow first.
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

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};

use serde::{Deserialize, Serialize};

use crate::events::{Event, EventBus};
use crate::treasury::{PLAZA_OWNER, PostError, Treasury};
use crate::{ConnectedAgent, now_ms};

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

/// The most a project id may be, in bytes.
pub const MAX_PROJECT: usize = 64;

/// The most tasks one task may run after. A plan's task lists are short; the
/// bound keeps a bad request cheap to check.
pub const MAX_AFTER: usize = 32;

/// The states noded v0 gives a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    /// Created and claimable.
    Open,
    /// Bound to a live agent.
    Claimed,
    /// The claiming agent reports the work done; it waits for a reviewer's
    /// verdict, and its escrow stays held until the verdict ends the task.
    AwaitingReview,
    /// A review sent the work back to its worker: the claim, the escrow and the worker's lane stay, and the
    /// worker's next done brings it back to `awaiting_review` for the next round.
    Rework,
    /// Ended: the work is accepted.
    Done,
    /// Ended: the agent reports the work failed.
    Failed,
    /// Ended: given up before it finished.
    Withdrawn,
}

impl TaskState {
    /// Every state, in the order a task moves through them.
    pub const ALL: [TaskState; 7] = [
        TaskState::Open,
        TaskState::Claimed,
        TaskState::AwaitingReview,
        TaskState::Rework,
        TaskState::Done,
        TaskState::Failed,
        TaskState::Withdrawn,
    ];

    /// The name the API and the schema's equivalents use.
    pub fn as_str(self) -> &'static str {
        match self {
            TaskState::Open => "open",
            TaskState::Claimed => "claimed",
            TaskState::AwaitingReview => "awaiting_review",
            TaskState::Rework => "rework",
            TaskState::Done => "done",
            TaskState::Failed => "failed",
            TaskState::Withdrawn => "withdrawn",
        }
    }

    /// Whether `self -> next` is one of the transitions v0 allows: a task is
    /// claimed from `open`, a claim the agent reports done waits in
    /// `awaiting_review`, a claim or a waiting task is finished (done or
    /// failed), and a task is withdrawn from any state before it ends. A claim
    /// goes back to `open` when the owner takes the workstation its worker was
    /// using ([`Task::requeue`]). Every other pair is refused.
    pub fn can_transition_to(self, next: TaskState) -> bool {
        matches!(
            (self, next),
            (TaskState::Open, TaskState::Claimed)
                | (TaskState::Claimed, TaskState::Open)
                | (TaskState::Claimed, TaskState::AwaitingReview)
                | (TaskState::Claimed, TaskState::Done)
                | (TaskState::Claimed, TaskState::Failed)
                | (TaskState::AwaitingReview, TaskState::Done)
                | (TaskState::AwaitingReview, TaskState::Failed)
                | (TaskState::AwaitingReview, TaskState::Rework)
                | (TaskState::Rework, TaskState::AwaitingReview)
                | (TaskState::Rework, TaskState::Failed)
                | (TaskState::Rework, TaskState::Withdrawn)
                | (TaskState::Open, TaskState::Withdrawn)
                | (TaskState::Claimed, TaskState::Withdrawn)
                | (TaskState::AwaitingReview, TaskState::Withdrawn)
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
            TaskState::Open
            | TaskState::Claimed
            | TaskState::AwaitingReview
            | TaskState::Rework => None,
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// The task is open but not ready: it runs after tasks that are not done.
    /// No task is worked on before everything it runs after is accepted
    /// (`TaskLifecycle.tla`'s `DepsAcceptedBeforeWork`).
    Waiting {
        /// The task's id.
        task: String,
        /// The tasks it runs after that are not done yet.
        waiting_on: Vec<String>,
        /// The tasks it runs after that ended without being done (failed or
        /// withdrawn).
        blocked_by: Vec<String>,
    },
}

impl ClaimError {
    /// The HTTP status this error is reported as. Every refusal is a conflict
    /// with the node's state (no live agent, a task that is not open, or one
    /// that waits on others), not a malformed request.
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
            ClaimError::Waiting {
                task,
                waiting_on,
                blocked_by,
            } => {
                let mut parts = Vec::new();
                if !waiting_on.is_empty() {
                    let verb = if waiting_on.len() == 1 { "is" } else { "are" };
                    parts.push(format!("{}, which {verb} not done yet", names(waiting_on)));
                }
                if !blocked_by.is_empty() {
                    parts.push(format!(
                        "{}, which ended without being done",
                        names(blocked_by)
                    ));
                }
                write!(formatter, "task {task} runs after {}", parts.join(", and "))
            }
        }
    }
}

impl std::error::Error for ClaimError {}

/// Why a task's links could not be changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    /// No task has that id.
    NotFound(String),
    /// The links are outside the ledger's contract: a task that does not
    /// exist, one in another project, one named twice, or more than
    /// [`MAX_AFTER`].
    Invalid(String),
    /// The links would close a cycle: the ids around it, from the task back to
    /// itself, each running after the next.
    Cycle(Vec<String>),
    /// The task is not open: a claimed or finished task keeps the links it
    /// had.
    NotOpen {
        /// The task's id.
        task: String,
        /// The state it is in.
        state: TaskState,
    },
}

impl LinkError {
    /// The HTTP status this error is reported as: an unknown task is not
    /// found, links outside the contract (a cycle among them) are a bad
    /// request, and a task that is no longer open is a conflict with its state.
    pub fn status(&self) -> u16 {
        match self {
            LinkError::NotFound(_) => 404,
            LinkError::Invalid(_) | LinkError::Cycle(_) => 400,
            LinkError::NotOpen { .. } => 409,
        }
    }
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::NotFound(id) => write!(formatter, "no task with id {id}"),
            LinkError::Invalid(reason) => write!(formatter, "{reason}"),
            LinkError::Cycle(cycle) => match cycle.split_first() {
                Some((first, rest)) => write!(
                    formatter,
                    "the links would close a cycle: {first} runs after {}",
                    rest.join(", which runs after ")
                ),
                None => write!(formatter, "the links would close a cycle"),
            },
            LinkError::NotOpen { task, state } => write!(
                formatter,
                "task {task} is {}: only an open task's links can change",
                state.as_str()
            ),
        }
    }
}

impl std::error::Error for LinkError {}

/// Why a task outside projects could not move into one ([`TaskLedger::adopt`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdoptError {
    /// No task has that id.
    NotFound(String),
    /// The project id is outside the ledger's contract.
    Invalid(String),
    /// The task is already in a project, which it never leaves.
    InProject {
        /// The task's id.
        task: String,
        /// The project it is in.
        project: String,
    },
}

impl AdoptError {
    /// The HTTP status this error is reported as.
    pub fn status(&self) -> u16 {
        match self {
            AdoptError::NotFound(_) => 404,
            AdoptError::Invalid(_) => 400,
            AdoptError::InProject { .. } => 409,
        }
    }
}

impl std::fmt::Display for AdoptError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdoptError::NotFound(id) => write!(formatter, "no task with id {id}"),
            AdoptError::Invalid(reason) => write!(formatter, "{reason}"),
            AdoptError::InProject { task, project } => write!(
                formatter,
                "task {task} is in project {project}: a task never leaves its project"
            ),
        }
    }
}

impl std::error::Error for AdoptError {}

/// Task ids as a sentence lists them: `t1`, `t1 and t2`, `t1, t2 and t3`.
fn names(ids: &[String]) -> String {
    match ids {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

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

/// The escrow a task gets when its poster names no budget. Every caller that
/// creates a task on a poster's behalf (the HTTP route, the agents' tools and
/// the CLI, which omits the field) gets this default through [`resolve_budget`].
pub const DEFAULT_TASK_BUDGET: i64 = 2000;

/// The smallest budget a poster may name, in credits: a budget is the escrow
/// that pays the worker's model calls and tool steps, and a few credits stop
/// the work at its first call.
pub const MIN_TASK_BUDGET: i64 = 200;

/// The budget a poster's request gets: the default when none is named, the
/// named budget when it is at least [`MIN_TASK_BUDGET`], and otherwise a refusal
/// that names the minimum (0 is below it, so it is refused too).
pub fn resolve_budget(requested: Option<u64>) -> Result<u64, String> {
    let minimum = MIN_TASK_BUDGET as u64;
    match requested {
        None => Ok(DEFAULT_TASK_BUDGET as u64),
        Some(budget) if budget < minimum => Err(format!(
            "a budget of {budget} credits is below the minimum of {MIN_TASK_BUDGET}: a task's budget is the escrow that pays the worker's model calls and tool steps (not a fee), so use at least {MIN_TASK_BUDGET} (the default is {DEFAULT_TASK_BUDGET}; typical work needs 1000-5000)"
        )),
        Some(budget) => Ok(budget),
    }
}

/// The `POST /v1/tasks` body.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateTask {
    /// What the task is for (the schema's `title`), as the caller wrote it.
    pub goal: String,
    /// How the work will be judged complete (the schema's `spec_ref`).
    #[serde(default)]
    pub acceptance: Option<String>,
    /// The task's budget, in credits. Absent is 0 at the ledger, which posts no
    /// escrow for it; the HTTP route and the agents' tools first apply
    /// [`resolve_budget`], so a poster's task is never unfunded by accident.
    #[serde(default)]
    pub budget: Option<u64>,
    /// The role the task is for, when it names one. v0 records it; it does not
    /// gate a claim on it (the assignment engine that would is a later slice).
    #[serde(default)]
    pub role: Option<String>,
    /// The tasks this one runs after, by id (the schema's `task_deps`): it is
    /// ready to be claimed once every one of them is done. Each must exist
    /// already and be in this task's project (or, for a task in no project, in
    /// none); at most [`MAX_AFTER`], none named twice.
    #[serde(default)]
    pub after: Vec<String>,
    /// The project the task is for (`pr<n>`), when it is one's. The ledger
    /// keeps the id, so a task names its project; apid checks that the project
    /// exists and its plan is approved, and links the task to it.
    #[serde(default)]
    pub project: Option<String>,
}

/// A create request's fields, checked.
struct Checked {
    goal: String,
    acceptance: Option<String>,
    budget: u64,
    role: Option<String>,
    after: Vec<String>,
    project: Option<String>,
}

impl CreateTask {
    /// Checks the request and returns the fields a task keeps, or says why the
    /// request is outside the ledger's contract. Whether the tasks `after`
    /// names exist, and in which project, is the ledger's check ([`resolve`]).
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
            after: checked_after(&self.after)?,
            project: optional_text("project", &self.project, MAX_PROJECT)?,
        })
    }
}

/// One task of a batch ([`TaskLedger::create_all`]).
#[derive(Debug, Clone)]
pub struct BatchTask {
    /// The task as a create takes it: its `after` names tasks that exist already.
    pub request: CreateTask,
    /// The earlier tasks of the same batch it runs after, by their place in the batch.
    pub after_batch: Vec<usize>,
}

/// A task's links, checked for what needs no ledger: at most [`MAX_AFTER`],
/// and no task named twice.
fn checked_after(after: &[String]) -> Result<Vec<String>, String> {
    if after.len() > MAX_AFTER {
        return Err(format!(
            "after may name at most {MAX_AFTER} tasks, not {}",
            after.len()
        ));
    }
    let mut seen = HashSet::new();
    if let Some(repeat) = after.iter().find(|id| !seen.insert(id.as_str())) {
        return Err(format!("after names {repeat} twice"));
    }
    Ok(after.to_vec())
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
pub(crate) fn optional_text(
    name: &str,
    value: &Option<String>,
    max: usize,
) -> Result<Option<String>, String> {
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
    /// The tasks this one runs after, in the order they were named. They
    /// change only while the task is open ([`TaskLedger::relink`]); a claimed
    /// or finished task keeps the links it had.
    after: Vec<Link>,
    /// The tasks that run after this one: the other end of their links, kept
    /// by the ledger as links are made and changed. When this task ends, each
    /// of them that is still open is announced again, since it may now be
    /// ready, or blocked. Weak, so a task does not keep the tasks after it.
    dependents: Vec<Weak<Task>>,
}

/// A task another one runs after. The link holds the task itself, so a task
/// reads whether it is ready without going through the ledger.
///
/// The locks: a task reads its links' states while it holds its own lock (to
/// build its view, or to claim), so one task's lock is only ever waited for
/// under another's along a link, from a task to one it runs after. Links never
/// close a cycle, so those waits cannot either. The ledger's lock is taken
/// before a task's, never under one.
#[derive(Clone)]
struct Link(Arc<Task>);

impl std::fmt::Debug for Link {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The id alone: the linked task's own links would repeat down the graph.
        formatter.write_str(&self.0.id)
    }
}

/// The ids of the tasks `after` links to, in order.
fn link_ids(after: &[Link]) -> Vec<String> {
    after.iter().map(|Link(task)| task.id.clone()).collect()
}

/// Announces each of `dependents` that still exists and is open. The caller
/// holds no task's lock: a dependent's event reads the state of the task that
/// just ended.
fn announce(dependents: &[Weak<Task>]) {
    for dependent in dependents.iter().filter_map(Weak::upgrade) {
        dependent.announce();
    }
}

/// Where a task's links stand.
#[derive(Default)]
struct LinkStates {
    /// The linked tasks not done yet: open, claimed or awaiting review.
    waiting_on: Vec<String>,
    /// The linked tasks that ended without being done: failed or withdrawn.
    blocked_by: Vec<String>,
}

impl LinkStates {
    /// Each link's state, read under the linked task's own lock.
    fn of(after: &[Link]) -> Self {
        let mut states = Self::default();
        for Link(task) in after {
            match task.lock().state {
                TaskState::Done => {}
                TaskState::Failed | TaskState::Withdrawn => states.blocked_by.push(task.id.clone()),
                TaskState::Open
                | TaskState::Claimed
                | TaskState::AwaitingReview
                | TaskState::Rework => {
                    states.waiting_on.push(task.id.clone());
                }
            }
        }
        states
    }

    /// Whether every link is done.
    fn all_done(&self) -> bool {
        self.waiting_on.is_empty() && self.blocked_by.is_empty()
    }
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
    /// The project the task is for, when it is one's: named when it was created, or set once when a task
    /// outside projects moves into one ([`TaskLedger::adopt`]). A task never leaves its project.
    project: OnceLock<String>,
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
    /// The project the task is for, or `null`.
    pub project: Option<&'a str>,
    /// The ids of the tasks this one runs after.
    pub after: Vec<String>,
    /// The task's state.
    pub state: &'static str,
    /// Whether the task can be claimed now: it is open and every task in
    /// `after` is done.
    pub ready: bool,
    /// While the task is open: the tasks in `after` not done yet.
    pub waiting_on: Vec<String>,
    /// While the task is open: the tasks in `after` that failed or were
    /// withdrawn. The task stays open; it waits to be relinked or withdrawn.
    pub blocked_by: Vec<String>,
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

    /// The project the task is for, when it is one's.
    pub fn project(&self) -> Option<&str> {
        self.project.get().map(String::as_str)
    }

    /// The ids of the tasks this one runs after, in the order they were named.
    pub fn after(&self) -> Vec<String> {
        link_ids(&self.lock().after)
    }

    /// Whether the task can be claimed now: it is open and every task it runs
    /// after is done (a task with no links is ready while it is open). The
    /// scheduler hands out ready tasks only, and a claim on an open task that
    /// is not ready is refused ([`ClaimError::Waiting`]).
    pub fn is_ready(&self) -> bool {
        let inner = self.lock();
        inner.state == TaskState::Open && LinkStates::of(&inner.after).all_done()
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
        let open = inner.state == TaskState::Open;
        // The links' states matter while the task waits to be claimed. A
        // claimed task's links were all done when it was claimed, and done is
        // final.
        let links = if open {
            LinkStates::of(&inner.after)
        } else {
            LinkStates::default()
        };
        TaskView {
            id: &self.id,
            goal: &self.goal,
            acceptance: self.acceptance.as_deref(),
            budget: self.budget,
            role: self.role.as_deref(),
            project: self.project(),
            after: link_ids(&inner.after),
            state: inner.state.as_str(),
            ready: open && links.all_done(),
            waiting_on: links.waiting_on,
            blocked_by: links.blocked_by,
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
    /// Only a ready task can be claimed: an `open` one whose links are all
    /// done. A second claim on a claimed task is refused by the state machine,
    /// like every other transition, and a claim on an open task that still
    /// waits on its links is refused with what it waits on. A refused claim
    /// changes nothing.
    pub fn claim(&self, agent: &ConnectedAgent) -> Result<(), ClaimError> {
        let mut inner = self.lock();
        let from = inner.state;
        if !from.can_transition_to(TaskState::Claimed) {
            return Err(ClaimError::Refused(RefusedTransition {
                from,
                to: TaskState::Claimed,
            }));
        }
        let links = LinkStates::of(&inner.after);
        if !links.all_done() {
            return Err(ClaimError::Waiting {
                task: self.id.clone(),
                waiting_on: links.waiting_on,
                blocked_by: links.blocked_by,
            });
        }
        let assignee = agent.assignee_id();
        inner.state = TaskState::Claimed;
        inner.assignee_id = Some(assignee);
        inner.agent_cid = Some(agent.cid);
        inner.updated_at = now_ms();
        self.events.publish(self.event(&inner));
        Ok(())
    }

    /// Puts a claimed task back in the queue: the owner took the workstation its
    /// worker was using, so the claim is released and the task is `open` to be
    /// claimed again. The assignee and the claiming CID are cleared, and `note`
    /// (why it was requeued) is kept as the task's note. Nothing is refunded:
    /// the escrow stays with the task, which is still going to be done.
    pub fn requeue(&self, note: &str) -> Result<(), RefusedTransition> {
        let to = TaskState::Open;
        let mut inner = self.lock();
        let from = inner.state;
        if !from.can_transition_to(to) {
            return Err(RefusedTransition { from, to });
        }
        inner.state = to;
        inner.assignee_id = None;
        inner.agent_cid = None;
        inner.note = Some(note.to_string());
        inner.updated_at = now_ms();
        self.events.publish(self.event(&inner));
        Ok(())
    }

    /// Moves a claimed task to `awaiting_review`: the claiming agent reports the
    /// work done, and it waits for a reviewer's verdict. `note` is the agent's
    /// report; the reviewer reads it from the task.
    ///
    /// Nothing is refunded. The escrow stays held while the task waits, so a
    /// done claim that no reviewer rules on pays nothing back to the budget until
    /// a verdict (or the owner's ruling) finishes it.
    pub fn await_review(&self, note: Option<String>) -> Result<(), RefusedTransition> {
        let to = TaskState::AwaitingReview;
        let mut inner = self.lock();
        let from = inner.state;
        if !from.can_transition_to(to) {
            return Err(RefusedTransition { from, to });
        }
        inner.state = to;
        inner.note = note;
        inner.updated_at = now_ms();
        self.events.publish(self.event(&inner));
        Ok(())
    }

    /// Sends a task awaiting review back to its worker (`rework`), with the review's findings as its note. The
    /// claim and the escrow stay.
    pub fn rework(&self, note: Option<String>) -> Result<(), RefusedTransition> {
        let to = TaskState::Rework;
        let mut inner = self.lock();
        let from = inner.state;
        if !from.can_transition_to(to) {
            return Err(RefusedTransition { from, to });
        }
        inner.state = to;
        inner.note = note;
        inner.updated_at = now_ms();
        self.events.publish(self.event(&inner));
        Ok(())
    }

    /// Ends the task as `outcome`, with what the agent has to say about it.
    ///
    /// Only a claimed task, or one awaiting review, can be finished. The claiming agent's CID is
    /// recorded at claim time; nothing in a finish request identifies the
    /// caller, so v0 records who claimed the task rather than proving who
    /// finished it (see the module note on the identity slice).
    ///
    /// Ending a task releases its escrow: what the task's escrow still holds
    /// goes back to the mint (a `refund` entry). The refund is posted **before**
    /// the state is committed, so a treasury that refuses it leaves the task in
    /// the state it had — a terminal task is never left holding its budget.
    pub fn finish(&self, outcome: FinishOutcome, note: Option<String>) -> Result<(), EndError> {
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
        self.events.publish(self.event(&inner));
        let dependents = inner.dependents.clone();
        drop(inner);
        announce(&dependents);
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
        self.events.publish(self.event(&inner));
        let dependents = inner.dependents.clone();
        drop(inner);
        announce(&dependents);
        Ok(())
    }

    /// The `task` event for this task as `inner` holds it: its state, its
    /// claim, its project and whether it is ready. Built under the task's own
    /// lock, so an open task reads its links' states as its view does.
    fn event(&self, inner: &TaskInner) -> Event {
        Event::Task {
            id: self.id.clone(),
            state: inner.state.as_str(),
            assignee: inner.assignee_id.clone(),
            agent_cid: inner.agent_cid,
            project: self.project.get().cloned(),
            ready: inner.state == TaskState::Open && LinkStates::of(&inner.after).all_done(),
        }
    }

    /// Publishes this task's `task` event again while it is open: a task it
    /// runs after ended, or its links changed, so it may now be ready or
    /// blocked.
    fn announce(&self) {
        let inner = self.lock();
        if inner.state == TaskState::Open {
            self.events.publish(self.event(&inner));
        }
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
        let treasury = Arc::new(Treasury::new(Arc::clone(&events)));
        Self::with_treasury(events, treasury)
    }

    /// A ledger over `treasury`, which may be one restored from a snapshot.
    pub fn with_treasury(events: Arc<EventBus>, treasury: Arc<Treasury>) -> Self {
        Self {
            treasury,
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
        // The links name tasks that exist already, so a new task cannot close
        // a cycle: nothing runs after it yet.
        let after = resolve(&inner.tasks, &checked.after, checked.project.as_deref())
            .map_err(CreateError::Invalid)?;
        let upstreams: Vec<Arc<Task>> = after.iter().map(|Link(task)| Arc::clone(task)).collect();
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
                    .fund_task_from(&id, amount, &self.budget_for(checked.project.as_deref()))
                    .map_err(CreateError::Treasury)?,
            );
        }
        inner.next_seq += 1;
        let task = self.open_task(id, checked, entry, after, now_ms());
        inner.tasks.push(Arc::clone(&task));
        // Each task it runs after learns of it, so it is announced when that
        // one ends.
        for upstream in &upstreams {
            upstream.lock().dependents.push(Arc::downgrade(&task));
        }
        // The task is registered under the ledger's lock, so a subscriber sees
        // it appear exactly once and in order.
        let event = task.event(&task.lock());
        self.events.publish(event);
        Ok(task)
    }

    /// Creates every task of `batch`, in order, or none of them: a project's whole task list,
    /// posted at once. Each request is held to the rules a create holds it to, and its
    /// `after_batch` names earlier tasks of the same batch, in its own project, that it runs after.
    /// It all happens under the ledger's lock, so no claim and no subscriber sees part of a batch.
    ///
    /// Every budget is minted into its task's escrow before any task is registered. If the
    /// treasury refuses one, what the batch had minted goes back to the plaza's budget, the ids
    /// whose escrows were funded are not handed out again (the escrows keep the mint and the
    /// refund as their history), and no task is created.
    pub fn create_all(&self, batch: &[BatchTask]) -> Result<Vec<Arc<Task>>, CreateError> {
        let mut checked: Vec<Checked> = Vec::with_capacity(batch.len());
        for (at, item) in batch.iter().enumerate() {
            let place = |reason: String| {
                CreateError::Invalid(format!("task {} of the batch: {reason}", at + 1))
            };
            let task = item.request.checked().map_err(place)?;
            let mut seen = HashSet::new();
            for &from in &item.after_batch {
                if from >= at {
                    return Err(place(format!(
                        "it may run after an earlier task of the batch only, not task {}",
                        from + 1
                    )));
                }
                if !seen.insert(from) {
                    return Err(place(format!(
                        "it names task {} of the batch twice",
                        from + 1
                    )));
                }
                if checked[from].project != task.project {
                    return Err(place(format!(
                        "task {} of the batch is in {}, and this task is in {}: a task runs after tasks of its own project only",
                        from + 1,
                        in_project(checked[from].project.as_deref()),
                        in_project(task.project.as_deref())
                    )));
                }
            }
            let links = task.after.len() + item.after_batch.len();
            if links > MAX_AFTER {
                return Err(place(format!(
                    "after may name at most {MAX_AFTER} tasks, not {links}"
                )));
            }
            checked.push(task);
        }
        let mut inner = self.lock();
        let mut existing = Vec::with_capacity(batch.len());
        for (at, task) in checked.iter().enumerate() {
            existing.push(
                resolve(&inner.tasks, &task.after, task.project.as_deref()).map_err(|reason| {
                    CreateError::Invalid(format!("task {} of the batch: {reason}", at + 1))
                })?,
            );
        }
        if inner.tasks.len() + batch.len() > MAX_TASKS {
            return Err(CreateError::Full { max: MAX_TASKS });
        }
        let first = inner.next_seq + 1;
        let ids: Vec<String> = (0..batch.len() as u64)
            .map(|at| format!("t{}", first + at))
            .collect();
        let mut entries: Vec<Option<u64>> = Vec::with_capacity(batch.len());
        for (task, id) in checked.iter().zip(&ids) {
            if task.budget == 0 {
                entries.push(None);
                continue;
            }
            // `checked` bounds the budget to `i64::MAX`.
            let amount = i64::try_from(task.budget).unwrap_or(i64::MAX);
            match self.treasury.fund_task_from(
                id,
                amount,
                &self.budget_for(task.project.as_deref()),
            ) {
                Ok(entry) => entries.push(Some(entry)),
                Err(refused) => {
                    for (funded, entry) in ids.iter().zip(&entries) {
                        if entry.is_some() {
                            // Best effort: a refund refused too leaves that escrow held.
                            let _ = self.treasury.refund_task_to_budget(funded);
                        }
                    }
                    let burned = entries
                        .iter()
                        .rposition(Option::is_some)
                        .map_or(0, |at| at + 1);
                    inner.next_seq += burned as u64;
                    return Err(CreateError::Treasury(refused));
                }
            }
        }
        inner.next_seq += batch.len() as u64;
        let now = now_ms();
        let mut created: Vec<Arc<Task>> = Vec::with_capacity(batch.len());
        let parts = batch
            .iter()
            .zip(checked)
            .zip(existing)
            .zip(ids.into_iter().zip(entries));
        for (((item, task), mut links), (id, entry)) in parts {
            links.extend(
                item.after_batch
                    .iter()
                    .map(|&from| Link(Arc::clone(&created[from]))),
            );
            let upstreams: Vec<Arc<Task>> = links
                .iter()
                .map(|Link(linked)| Arc::clone(linked))
                .collect();
            let new = self.open_task(id, task, entry, links, now);
            inner.tasks.push(Arc::clone(&new));
            for upstream in &upstreams {
                upstream.lock().dependents.push(Arc::downgrade(&new));
            }
            created.push(new);
        }
        // Announced once the whole batch is registered, in order.
        for task in &created {
            let event = task.event(&task.lock());
            self.events.publish(event);
        }
        Ok(created)
    }

    /// A new task in `open`, as a create registers it.
    fn open_task(
        &self,
        id: String,
        checked: Checked,
        entry: Option<u64>,
        after: Vec<Link>,
        now: u64,
    ) -> Arc<Task> {
        Arc::new(Task {
            id,
            goal: checked.goal,
            acceptance: checked.acceptance,
            budget: checked.budget,
            role: checked.role,
            project: checked.project.map(OnceLock::from).unwrap_or_default(),
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
                after,
                dependents: Vec::new(),
            }),
        })
    }

    /// The town budget a task of `project` is funded from: the project's own once its budget is set aside, else
    /// the plaza's.
    fn budget_for(&self, project: Option<&str>) -> String {
        match project {
            Some(project) if self.treasury.has_town_budget(project) => project.to_string(),
            _ => PLAZA_OWNER.to_string(),
        }
    }

    /// The task with that id, if there is one.
    pub fn find(&self, id: &str) -> Option<Arc<Task>> {
        self.lock().tasks.iter().find(|task| task.id == id).cloned()
    }

    /// Every task, oldest first.
    pub fn list(&self) -> Vec<Arc<Task>> {
        self.lock().tasks.clone()
    }

    /// Replaces the links of the open task `id` with `after`, held to the
    /// rules a create holds them to, and refused when they would close a cycle.
    /// Returns the task. Every link change holds the ledger's lock, so two
    /// relinks cannot close a cycle between them.
    pub fn relink(&self, id: &str, after: &[String]) -> Result<Arc<Task>, LinkError> {
        let after = checked_after(after).map_err(LinkError::Invalid)?;
        let inner = self.lock();
        let task = inner
            .tasks
            .iter()
            .find(|task| task.id == id)
            .cloned()
            .ok_or_else(|| LinkError::NotFound(id.to_string()))?;
        let links = resolve(&inner.tasks, &after, task.project()).map_err(LinkError::Invalid)?;
        let not_open = |state: TaskState| LinkError::NotOpen {
            task: id.to_string(),
            state,
        };
        let state = task.lock().state;
        if state != TaskState::Open {
            return Err(not_open(state));
        }
        // The walk takes the other tasks' locks one at a time, with this
        // task's lock not held: a view of a task that runs after this one holds
        // its own lock while it waits for this one's.
        if let Some(cycle) = cycle_through(&task, &links) {
            return Err(LinkError::Cycle(cycle));
        }
        let mut locked = task.lock();
        // The scheduler may have claimed it while the links were checked.
        if locked.state != TaskState::Open {
            return Err(not_open(locked.state));
        }
        let upstreams: Vec<Arc<Task>> = links
            .iter()
            .map(|Link(linked)| Arc::clone(linked))
            .collect();
        let old = std::mem::replace(&mut locked.after, links);
        locked.updated_at = now_ms();
        drop(locked);
        // The tasks it ran after forget it, and the ones it runs after now
        // learn of it, one lock at a time.
        let weak = Arc::downgrade(&task);
        for Link(upstream) in &old {
            upstream
                .lock()
                .dependents
                .retain(|dependent| !dependent.ptr_eq(&weak));
        }
        for upstream in &upstreams {
            upstream.lock().dependents.push(Weak::clone(&weak));
        }
        self.events.publish(Event::TaskLink {
            task: task.id.clone(),
            after: upstreams.iter().map(|linked| linked.id.clone()).collect(),
        });
        task.announce();
        Ok(task)
    }

    /// Moves task `id`, which is in no project, into `project`, together with every task linked to it: the tasks it
    /// runs after and the ones that run after it, transitively. Links stay inside a project, so a linked group moves
    /// as one, and every task of it is outside projects too. Returns the tasks moved, oldest first; each one's `task`
    /// event goes out again with its project.
    pub fn adopt(&self, id: &str, project: &str) -> Result<Vec<Arc<Task>>, AdoptError> {
        let project =
            required_text("project", project, MAX_PROJECT).map_err(AdoptError::Invalid)?;
        let inner = self.lock();
        let task = inner
            .tasks
            .iter()
            .find(|task| task.id == id)
            .cloned()
            .ok_or_else(|| AdoptError::NotFound(id.to_string()))?;
        if let Some(current) = task.project() {
            return Err(AdoptError::InProject {
                task: id.to_string(),
                project: current.to_string(),
            });
        }
        // The linked group, over links in both directions, one task's lock at a time.
        let mut group: Vec<Arc<Task>> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut queue = VecDeque::from([task]);
        while let Some(next) = queue.pop_front() {
            if !seen.insert(next.id.clone()) {
                continue;
            }
            let (upstream, downstream): (Vec<Arc<Task>>, Vec<Arc<Task>>) = {
                let locked = next.lock();
                (
                    locked
                        .after
                        .iter()
                        .map(|Link(linked)| Arc::clone(linked))
                        .collect(),
                    locked.dependents.iter().filter_map(Weak::upgrade).collect(),
                )
            };
            queue.extend(upstream);
            queue.extend(downstream);
            group.push(next);
        }
        for member in &group {
            // Unset on every member: the group is outside projects, and the ledger's lock is held.
            let _ = member.project.set(project.clone());
        }
        let order: HashMap<&str, usize> = inner
            .tasks
            .iter()
            .enumerate()
            .map(|(index, task)| (task.id.as_str(), index))
            .collect();
        group.sort_by_key(|task| order.get(task.id.as_str()).copied().unwrap_or(usize::MAX));
        drop(order);
        drop(inner);
        for member in &group {
            let event = member.event(&member.lock());
            self.events.publish(event);
        }
        Ok(group)
    }

    /// Of the tasks `after` names, the ones that are not done: what a task
    /// created with these links would wait on. An id that names no task is
    /// left out; the create refuses it.
    pub fn not_done(&self, after: &[String]) -> Vec<String> {
        let tasks = self.list();
        after
            .iter()
            .filter(|id| {
                tasks
                    .iter()
                    .any(|task| task.id == **id && task.lock().state != TaskState::Done)
            })
            .cloned()
            .collect()
    }

    fn lock(&self) -> MutexGuard<'_, LedgerInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The tasks `after` names, out of `tasks`, as links for a task in `project`:
/// each must exist, and be in the same project (or, for a task in no project,
/// in none).
fn resolve(
    tasks: &[Arc<Task>],
    after: &[String],
    project: Option<&str>,
) -> Result<Vec<Link>, String> {
    after
        .iter()
        .map(|id| {
            let task = tasks
                .iter()
                .find(|task| task.id == *id)
                .ok_or_else(|| format!("after names {id}, which is not a task"))?;
            if task.project() != project {
                return Err(format!(
                    "{id} is in {}, and this task is in {}: a task runs after tasks of its own project only",
                    in_project(task.project()),
                    in_project(project)
                ));
            }
            Ok(Link(Arc::clone(task)))
        })
        .collect()
}

/// Where a task is, as an error names it: `project pr2`, or `no project`.
fn in_project(project: Option<&str>) -> String {
    match project {
        Some(project) => format!("project {project}"),
        None => "no project".to_string(),
    }
}

/// The cycle `task` would close by running after `after`: the ids around it,
/// from `task` back to itself, each running after the next, or `None` when
/// the links close none.
///
/// A breadth-first walk along the links from the tasks `after` names, so the
/// cycle it reports is a shortest one. Each task's links are read under that
/// task's lock, one task at a time; the caller holds the ledger's lock, which
/// every link change takes, so the links cannot change during the walk.
fn cycle_through(task: &Task, after: &[Link]) -> Option<Vec<String>> {
    // Each task the walk reached, and the task it reached it from.
    let mut reached_from: HashMap<String, String> = HashMap::new();
    let mut queue: VecDeque<(String, Arc<Task>)> = after
        .iter()
        .map(|Link(next)| (task.id.clone(), Arc::clone(next)))
        .collect();
    while let Some((from, next)) = queue.pop_front() {
        if reached_from.contains_key(&next.id) {
            continue;
        }
        reached_from.insert(next.id.clone(), from);
        if next.id == task.id {
            // Back from `task` along the way the walk came, then turned around.
            let mut cycle = vec![task.id.clone()];
            let mut at = &reached_from[&task.id];
            while *at != task.id {
                cycle.push(at.clone());
                at = &reached_from[at];
            }
            cycle.push(task.id.clone());
            cycle.reverse();
            return Some(cycle);
        }
        let links = next.lock().after.clone();
        queue.extend(
            links
                .into_iter()
                .map(|Link(upstream)| (next.id.clone(), upstream)),
        );
    }
    None
}

/// The ledger's state as a snapshot holds it: the id counter and every task's
/// record, oldest first. The `Arc`s a live task holds (the bus and the treasury)
/// are not state, so they are not in it; restore hands them back.
#[derive(Debug, Serialize, Deserialize)]
struct LedgerSnapshot {
    next_seq: u64,
    tasks: Vec<TaskRecord>,
}

/// One task as a snapshot holds it. The outcome is not stored: it is the
/// state's own (see [`TaskState::outcome`]), so a record cannot disagree with
/// its state.
#[derive(Debug, Serialize, Deserialize)]
struct TaskRecord {
    id: String,
    goal: String,
    acceptance: Option<String>,
    budget: u64,
    role: Option<String>,
    escrow_entry: Option<u64>,
    created_at: u64,
    state: TaskState,
    assignee_id: Option<String>,
    verifier_id: Option<String>,
    agent_cid: Option<u64>,
    note: Option<String>,
    updated_at: u64,
    /// The task's project; none in a record from before tasks had one.
    #[serde(default)]
    project: Option<String>,
    /// The ids of the tasks it runs after; none in a record from before links.
    #[serde(default)]
    after: Vec<String>,
}

impl Task {
    /// The task's state as a snapshot record.
    fn record(&self) -> TaskRecord {
        let inner = self.lock();
        TaskRecord {
            id: self.id.clone(),
            goal: self.goal.clone(),
            acceptance: self.acceptance.clone(),
            budget: self.budget,
            role: self.role.clone(),
            escrow_entry: self.escrow_entry,
            created_at: self.created_at,
            state: inner.state,
            assignee_id: inner.assignee_id.clone(),
            verifier_id: inner.verifier_id.clone(),
            agent_cid: inner.agent_cid,
            note: inner.note.clone(),
            updated_at: inner.updated_at,
            project: self.project.get().cloned(),
            after: link_ids(&inner.after),
        }
    }

    /// Rebuilds a live task from its record, over the given bus and treasury,
    /// without its links: a link holds the task it names, so the ledger links
    /// the tasks once they are all rebuilt.
    fn from_record(record: TaskRecord, events: &Arc<EventBus>, treasury: &Arc<Treasury>) -> Self {
        Self {
            id: record.id,
            goal: record.goal,
            acceptance: record.acceptance,
            budget: record.budget,
            role: record.role,
            project: record.project.map(OnceLock::from).unwrap_or_default(),
            escrow_entry: record.escrow_entry,
            created_at: record.created_at,
            events: Arc::clone(events),
            treasury: Arc::clone(treasury),
            inner: Mutex::new(TaskInner {
                state: record.state,
                outcome: record.state.outcome(),
                assignee_id: record.assignee_id,
                agent_cid: record.agent_cid,
                verifier_id: record.verifier_id,
                note: record.note,
                updated_at: record.updated_at,
                after: Vec::new(),
                dependents: Vec::new(),
            }),
        }
    }
}

/// The invariants a ledger snapshot must hold before it may replace a ledger:
/// at most [`MAX_TASKS`] tasks, each with the canonical id `t<n>` and no id
/// repeated, and an id counter at least the largest `n`, so the next create
/// cannot hand out an id that is already taken. The links are held to the
/// rules a create holds them to: each names a task of the snapshot in the same
/// project, at most [`MAX_AFTER`] and none twice, and together they close no
/// cycle.
fn check_ledger_snapshot(snapshot: &LedgerSnapshot) -> Result<(), String> {
    if snapshot.tasks.len() > MAX_TASKS {
        return Err(format!(
            "the snapshot holds {} tasks, over the cap of {MAX_TASKS}",
            snapshot.tasks.len()
        ));
    }
    let mut seen = HashSet::new();
    let mut largest = 0;
    for record in &snapshot.tasks {
        let seq = record
            .id
            .strip_prefix('t')
            .and_then(|digits| digits.parse::<u64>().ok())
            .filter(|seq| record.id == format!("t{seq}"))
            .ok_or_else(|| format!("the snapshot's task id {:?} is not t<n>", record.id))?;
        if !seen.insert(seq) {
            return Err(format!("the snapshot repeats task id {}", record.id));
        }
        largest = largest.max(seq);
    }
    if largest > snapshot.next_seq {
        return Err(format!(
            "the snapshot's id counter {} is behind its largest id t{largest}",
            snapshot.next_seq
        ));
    }
    let projects: HashMap<&str, Option<&str>> = snapshot
        .tasks
        .iter()
        .map(|record| (record.id.as_str(), record.project.as_deref()))
        .collect();
    for record in &snapshot.tasks {
        checked_after(&record.after)
            .map_err(|reason| format!("the snapshot's task {}: {reason}", record.id))?;
        for id in &record.after {
            match projects.get(id.as_str()) {
                None => {
                    return Err(format!(
                        "the snapshot's task {} runs after {id}, which the snapshot does not hold",
                        record.id
                    ));
                }
                Some(project) if *project != record.project.as_deref() => {
                    return Err(format!(
                        "the snapshot's task {} runs after {id}, which is in another project",
                        record.id
                    ));
                }
                Some(_) => {}
            }
        }
    }
    if let Some(id) = never_ready(&snapshot.tasks) {
        return Err(format!(
            "the snapshot's links close a cycle: task {id} could never be ready"
        ));
    }
    Ok(())
}

/// A task the records' links keep from ever being ready, because a cycle runs
/// through it or through a task it runs after, or `None` when the links close
/// no cycle. Kahn's sort: a task is taken once every task it runs after is
/// taken, and what is left once nothing more can be taken is on a cycle or
/// behind one. The caller has checked that every link names a record, once.
fn never_ready(records: &[TaskRecord]) -> Option<&str> {
    let mut left: HashMap<&str, usize> = records
        .iter()
        .map(|record| (record.id.as_str(), record.after.len()))
        .collect();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    for record in records {
        for id in &record.after {
            dependents
                .entry(id.as_str())
                .or_default()
                .push(record.id.as_str());
        }
    }
    let mut takeable: Vec<&str> = records
        .iter()
        .filter(|record| record.after.is_empty())
        .map(|record| record.id.as_str())
        .collect();
    while let Some(id) = takeable.pop() {
        for dependent in dependents.get(id).into_iter().flatten() {
            if let Some(count) = left.get_mut(dependent) {
                *count -= 1;
                if *count == 0 {
                    takeable.push(dependent);
                }
            }
        }
    }
    records
        .iter()
        .map(|record| record.id.as_str())
        .find(|id| left.get(id).is_some_and(|count| *count > 0))
}

impl TaskLedger {
    /// The whole state as JSON: the id counter and every task's record. Restore
    /// reads it back with [`TaskLedger::restore`].
    pub fn snapshot(&self) -> String {
        let inner = self.lock();
        let snapshot = LedgerSnapshot {
            next_seq: inner.next_seq,
            tasks: inner.tasks.iter().map(|task| task.record()).collect(),
        };
        serde_json::to_string(&snapshot).expect("the ledger state serializes")
    }

    /// Replaces the state with a [`TaskLedger::snapshot`]; a refusal changes
    /// nothing. The tasks are rebuilt over this ledger's own bus and treasury,
    /// and neither is touched: a restore posts nothing and publishes nothing.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let snapshot: LedgerSnapshot = serde_json::from_str(json)
            .map_err(|err| format!("the snapshot does not parse: {err}"))?;
        check_ledger_snapshot(&snapshot)?;
        // The tasks first, then their links: a record names its links by id,
        // and a link holds the task it names.
        let mut after = Vec::with_capacity(snapshot.tasks.len());
        let tasks: Vec<Arc<Task>> = snapshot
            .tasks
            .into_iter()
            .map(|mut record| {
                after.push(std::mem::take(&mut record.after));
                Arc::new(Task::from_record(record, &self.events, &self.treasury))
            })
            .collect();
        let by_id: HashMap<&str, &Arc<Task>> =
            tasks.iter().map(|task| (task.id.as_str(), task)).collect();
        for (task, ids) in tasks.iter().zip(after) {
            // `check_ledger_snapshot` found every id among the tasks.
            let links: Vec<Link> = ids
                .iter()
                .filter_map(|id| by_id.get(id.as_str()))
                .map(|linked| Link(Arc::clone(linked)))
                .collect();
            for Link(upstream) in &links {
                upstream.lock().dependents.push(Arc::downgrade(task));
            }
            task.lock().after = links;
        }
        let mut inner = self.lock();
        inner.next_seq = snapshot.next_seq;
        inner.tasks = tasks;
        Ok(())
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
                after: Vec::new(),
                project: None,
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
                after: Vec::new(),
                project: None,
            })
            .expect("a budgeted task inside the contract")
    }

    /// `core.task_transitions`, restricted to the states noded v0 implements
    /// (`awaiting_review` is the schema's `verifying`, and `rework` its `rework`), and the v0 requeue
    /// (`claimed` -> `open`), which the schema has no row for.
    const ALLOWED: [(TaskState, TaskState); 14] = [
        (TaskState::Open, TaskState::Claimed),
        (TaskState::Claimed, TaskState::Open),
        (TaskState::Claimed, TaskState::AwaitingReview),
        (TaskState::Claimed, TaskState::Done),
        (TaskState::Claimed, TaskState::Failed),
        (TaskState::AwaitingReview, TaskState::Done),
        (TaskState::AwaitingReview, TaskState::Failed),
        (TaskState::Open, TaskState::Withdrawn),
        (TaskState::Claimed, TaskState::Withdrawn),
        (TaskState::AwaitingReview, TaskState::Withdrawn),
        (TaskState::AwaitingReview, TaskState::Rework),
        (TaskState::Rework, TaskState::AwaitingReview),
        (TaskState::Rework, TaskState::Failed),
        (TaskState::Rework, TaskState::Withdrawn),
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
            ClaimError::Refused(RefusedTransition {
                from: TaskState::Claimed,
                to: TaskState::Claimed
            })
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
    fn a_requeue_puts_a_claimed_task_back_in_the_queue_and_only_a_claimed_one() {
        let ledger = TaskLedger::new(EventBus::new());
        let task = create(&ledger, "render the reel");
        assert!(
            task.requeue("owner took the workstation").is_err(),
            "an open task has no claim to release"
        );

        task.claim(&ConnectedAgent {
            cid: 100,
            role: Some("builder".to_string()),
            run: "r1".to_string(),
            vm: 2,
        })
        .expect("open -> claimed");
        task.requeue("owner took the workstation")
            .expect("claimed -> open");
        let view = task.view();
        assert_eq!(view.state, "open");
        assert_eq!(view.assignee_id, None);
        assert_eq!(view.agent_cid, None);
        assert_eq!(view.note.as_deref(), Some("owner took the workstation"));
        assert_eq!(view.outcome, None, "a requeued task has not ended");
        assert!(task.is_open(), "and it can be claimed again");

        task.claim(&ConnectedAgent {
            cid: 101,
            role: Some("builder".to_string()),
            run: "r2".to_string(),
            vm: 3,
        })
        .expect("claimed again by another agent");
        assert_eq!(task.view().agent_cid, Some(101));
        task.finish(FinishOutcome::Done, None)
            .expect("and it finishes");
        assert!(
            task.requeue("late").is_err(),
            "a finished task is not requeued"
        );
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
        assert_eq!(refused.to_string(), "a task may not go from open to done");
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
        assert_eq!(refused.to_string(), "a task may not go from done to failed");

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

        let refused = claimed.withdraw().expect_err("a withdrawn task is final");
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
                after: Vec::new(),
                project: None,
            })
            .expect_err("the ledger is full");
        assert_eq!(refused, CreateError::Full { max: MAX_TASKS });
        assert_eq!(refused.to_string(), "the task ledger is full (1024 tasks)");
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
                after: Vec::new(),
                project: None,
            })
            .expect_err("the withdrawn task still counts");
        assert_eq!(refused_again, CreateError::Full { max: MAX_TASKS });
    }

    #[test]
    fn a_request_outside_the_contract_creates_nothing() {
        let ledger = TaskLedger::new(EventBus::new());
        let request =
            |goal: &str, acceptance: Option<&str>, budget: Option<u64>, role: Option<&str>| {
                CreateTask {
                    goal: goal.to_string(),
                    acceptance: acceptance.map(String::from),
                    budget,
                    role: role.map(String::from),
                    after: Vec::new(),
                    project: None,
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

        ledger
            .treasury()
            .mint_plaza_budget(i64::MAX)
            .expect("the plaza covers the largest budget");
        // The edges are inside the contract.
        let edges = [
            request("g", None, None, None),
            request("g", Some("a"), Some(i64::MAX as u64), Some("builder")),
            request(
                &"g".repeat(MAX_TEXT),
                Some(&"a".repeat(MAX_TEXT)),
                Some(0),
                Some(&"r".repeat(MAX_ROLE)),
            ),
        ];
        let ids: Vec<String> = edges
            .iter()
            .map(|edge| {
                ledger
                    .create(edge)
                    .expect("inside the contract")
                    .id()
                    .to_string()
            })
            .collect();
        // The refused requests handed out no ids, so the first task is t1.
        assert_eq!(ids, vec!["t1", "t2", "t3"]);

        // The optional fields default to the schema's.
        let defaulted = ledger
            .create(&request("just a goal", None, None, None))
            .expect("a task");
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
                after: Vec::new(),
                project: None,
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
        assert_eq!(view["project"], serde_json::Value::Null);
        assert_eq!(view["after"], serde_json::json!([]));
        assert_eq!(view["ready"], false, "a done task is not ready: it is over");
        assert_eq!(view["waiting_on"], serde_json::json!([]));
        assert_eq!(view["blocked_by"], serde_json::json!([]));
        assert!(view["updated_at"].as_u64().expect("updated_at") >= created_at);

        let list = serde_json::to_value(TasksView {
            tasks: ledger.list().iter().map(|task| task.view()).collect(),
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
                "event: task\ndata: {\"id\":\"t1\",\"state\":\"open\",\"assignee\":null,\"agent_cid\":null,\"project\":null,\"ready\":true}\n\n",
                "event: task\ndata: {\"id\":\"t1\",\"state\":\"claimed\",\"assignee\":\"builder\",\"agent_cid\":100,\"project\":null,\"ready\":false}\n\n",
                "event: task\ndata: {\"id\":\"t1\",\"state\":\"done\",\"assignee\":\"builder\",\"agent_cid\":100,\"project\":null,\"ready\":false}\n\n",
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
            "event: task\ndata: {\"id\":\"t2\",\"state\":\"open\",\"assignee\":null,\"agent_cid\":null,\"project\":null,\"ready\":true}\n\n"
        );
        assert_eq!(
            receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("the withdrawn event")
                .frame(),
            "event: task\ndata: {\"id\":\"t2\",\"state\":\"withdrawn\",\"assignee\":null,\"agent_cid\":null,\"project\":null,\"ready\":false}\n\n"
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
    fn a_done_claim_waits_for_review_and_keeps_its_escrow_held() {
        let ledger = TaskLedger::new(EventBus::new());
        let task = budgeted(&ledger, "reviewed work", 80);
        task.claim(&agent(100, "builder")).expect("claim");

        // The worker reports done: the task waits, and nothing is refunded.
        task.await_review(Some("built it".to_string()))
            .expect("claimed -> awaiting_review");
        assert_eq!(task.state(), "awaiting_review");
        assert_eq!(task.view().note.as_deref(), Some("built it"));
        assert!(!task.is_open());
        assert_eq!(
            ledger.treasury().escrow_balance("t1"),
            80,
            "the escrow is still held"
        );
        assert_eq!(
            ledger.treasury().view().totals.entries,
            2,
            "no refund was posted"
        );

        // Only a verdict ends it, and the refund is what the escrow still holds.
        task.finish(FinishOutcome::Done, Some("pass".to_string()))
            .expect("awaiting_review -> done");
        assert_eq!(task.state(), "done");
        assert_eq!(ledger.treasury().escrow_balance("t1"), 0);
        assert_eq!(ledger.treasury().view().totals.entries, 3);
    }

    #[test]
    fn a_waiting_task_cannot_be_claimed_again_and_only_a_claim_can_wait() {
        let ledger = TaskLedger::new(EventBus::new());
        let task = budgeted(&ledger, "reviewed work", 10);
        let refused = task
            .await_review(None)
            .expect_err("open -> awaiting_review is refused");
        assert_eq!(
            refused.to_string(),
            "a task may not go from open to awaiting_review"
        );

        task.claim(&agent(100, "builder")).expect("claim");
        task.await_review(None).expect("claimed -> awaiting_review");
        assert!(
            task.claim(&agent(101, "builder")).is_err(),
            "waiting is not open"
        );
        assert_eq!(task.state(), "awaiting_review");
        assert_eq!(ledger.treasury().escrow_balance("t1"), 10);
    }

    #[test]
    fn withdrawn_and_terminal_tasks_refund_the_whole_escrow() {
        // Was a_waiting_task_can_be_withdrawn_and_then_refunds.
        {
            let ledger = TaskLedger::new(EventBus::new());
            let task = budgeted(&ledger, "given up", 25);
            task.claim(&agent(100, "builder")).expect("claim");
            task.await_review(None).expect("claimed -> awaiting_review");
            task.withdraw().expect("awaiting_review -> withdrawn");
            assert_eq!(task.state(), "withdrawn");
            assert_eq!(ledger.treasury().escrow_balance("t1"), 0);
        }
        // Was every_terminal_state_refunds_the_whole_escrow.
        {
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
    }

    #[test]
    fn a_poster_budget_defaults_and_refuses_below_the_minimum() {
        assert_eq!(resolve_budget(None), Ok(2000));
        assert_eq!(resolve_budget(Some(200)), Ok(200));
        assert_eq!(resolve_budget(Some(5000)), Ok(5000));
        for low in [0, 1, 199] {
            let refusal = resolve_budget(Some(low)).expect_err("below the minimum");
            assert!(
                refusal.contains("minimum of 200"),
                "names the minimum: {refusal}"
            );
            assert!(
                refusal.contains(&format!("{low} credits")),
                "names the budget: {refusal}"
            );
        }
        assert_eq!(DEFAULT_TASK_BUDGET, 2000);
        assert_eq!(MIN_TASK_BUDGET, 200);
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
                .open_account(
                    crate::treasury::AccountKind::TownBudget,
                    &format!("town{index}"),
                    0,
                )
                .expect("an account under the cap");
        }
        let refused = ledger
            .create(&CreateTask {
                goal: "cannot be funded".to_string(),
                acceptance: None,
                budget: Some(5),
                role: None,
                after: Vec::new(),
                project: None,
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

    /// The task views a ledger lists, as JSON: the API's own shape, compared whole.
    fn views(ledger: &TaskLedger) -> Vec<serde_json::Value> {
        ledger
            .list()
            .iter()
            .map(|task| serde_json::to_value(task.view()).expect("a view serializes"))
            .collect()
    }

    #[test]
    fn a_snapshot_restores_the_same_task_views_over_the_same_treasury() {
        let events = EventBus::new();
        let ledger = TaskLedger::new(Arc::clone(&events));
        let open = budgeted(&ledger, "still open", 10);
        assert_eq!(open.id(), "t1");
        let claimed = budgeted(&ledger, "claimed", 20);
        claimed
            .claim(&agent(100, "builder"))
            .expect("open -> claimed");
        let done = budgeted(&ledger, "done", 30);
        done.claim(&agent(101, "builder")).expect("open -> claimed");
        done.finish(FinishOutcome::Done, Some("shipped".to_string()))
            .expect("claimed -> done");
        let failed = create(&ledger, "failed");
        failed
            .claim(&agent(102, "tester"))
            .expect("open -> claimed");
        failed
            .finish(FinishOutcome::Failed, None)
            .expect("claimed -> failed");
        let withdrawn = create(&ledger, "withdrawn");
        withdrawn.withdraw().expect("open -> withdrawn");

        let before = views(&ledger);
        assert_eq!(before.len(), 5);
        let json = ledger.snapshot();

        // Restored over the same treasury, as the daemon does: the treasury is
        // restored first and the ledger keeps its link to it.
        let treasury = Arc::clone(ledger.treasury());
        let version = treasury.version();
        let restored = TaskLedger::with_treasury(Arc::clone(&events), Arc::clone(&treasury));
        restored
            .restore(&json)
            .expect("a snapshot of a ledger restores");
        assert_eq!(views(&restored), before);
        assert_eq!(
            treasury.version(),
            version,
            "a restore posts nothing to the treasury"
        );
        assert!(Arc::ptr_eq(restored.treasury(), &treasury));
        // The id counter came back too: the next task is t6, never a reused id.
        assert_eq!(create(&restored, "next").id(), "t6");
    }

    #[test]
    fn a_garbage_snapshot_is_refused_and_changes_nothing() {
        let ledger = TaskLedger::new(EventBus::new());
        let kept = create(&ledger, "keep me");
        kept.claim(&agent(100, "builder")).expect("open -> claimed");
        let before = views(&ledger);
        let value: serde_json::Value =
            serde_json::from_str(&ledger.snapshot()).expect("the snapshot is JSON");

        let mut duplicated = value.clone();
        let first = duplicated["tasks"][0].clone();
        duplicated["tasks"]
            .as_array_mut()
            .expect("a task list")
            .push(first);
        let mut behind = value.clone();
        behind["next_seq"] = serde_json::json!(0);

        for garbage in [
            "not json".to_string(),
            "{\"tasks\": 3}".to_string(),
            duplicated.to_string(),
            behind.to_string(),
        ] {
            assert!(ledger.restore(&garbage).is_err(), "{garbage}");
            assert_eq!(views(&ledger), before, "a refused restore changes nothing");
        }
    }

    /// A task that runs after `after`, in `project`.
    fn linked(
        ledger: &TaskLedger,
        goal: &str,
        after: &[&str],
        project: Option<&str>,
    ) -> Result<Arc<Task>, CreateError> {
        ledger.create(&CreateTask {
            goal: goal.to_string(),
            acceptance: None,
            budget: None,
            role: None,
            after: after.iter().map(|id| id.to_string()).collect(),
            project: project.map(str::to_string),
        })
    }

    /// Claims `task` for a builder and finishes it as `outcome`.
    fn run(task: &Task, outcome: FinishOutcome) {
        task.claim(&agent(100, "builder"))
            .expect("a ready task is claimed");
        task.finish(outcome, None).expect("a claim finishes");
    }

    #[test]
    fn a_task_is_ready_once_every_task_it_runs_after_is_done() {
        let ledger = TaskLedger::new(EventBus::new());
        let schema = create(&ledger, "write the schema");
        let api = create(&ledger, "write the API");
        let console = linked(&ledger, "build the console", &["t1", "t2"], None).expect("linked");

        let view = console.view();
        assert_eq!(view.after, vec!["t1", "t2"]);
        assert!(!view.ready);
        assert_eq!(view.waiting_on, vec!["t1", "t2"]);
        assert!(view.blocked_by.is_empty());
        assert!(!console.is_ready());
        assert!(
            schema.is_ready() && api.is_ready(),
            "a task with no links is ready"
        );

        let refused = console
            .claim(&agent(101, "builder"))
            .expect_err("a task is not claimed before its links are done");
        assert_eq!(
            refused,
            ClaimError::Waiting {
                task: "t3".to_string(),
                waiting_on: vec!["t1".to_string(), "t2".to_string()],
                blocked_by: Vec::new(),
            }
        );
        assert_eq!(refused.status(), 409);
        assert_eq!(
            refused.to_string(),
            "task t3 runs after t1 and t2, which are not done yet"
        );
        assert_eq!(
            console.view().state,
            "open",
            "a refused claim changes nothing"
        );
        assert_eq!(console.view().agent_cid, None);

        run(&schema, FinishOutcome::Done);
        assert_eq!(console.view().waiting_on, vec!["t2"]);

        // Claimed and awaiting review are not done: done means accepted.
        api.claim(&agent(102, "builder")).expect("claim");
        assert_eq!(console.view().waiting_on, vec!["t2"]);
        api.await_review(Some("built".to_string()))
            .expect("claimed -> awaiting_review");
        assert!(!console.is_ready());
        assert_eq!(
            console
                .claim(&agent(101, "builder"))
                .unwrap_err()
                .to_string(),
            "task t3 runs after t2, which is not done yet"
        );
        api.finish(FinishOutcome::Done, Some("pass".to_string()))
            .expect("awaiting_review -> done");

        let view = console.view();
        assert!(view.ready);
        assert!(view.waiting_on.is_empty() && view.blocked_by.is_empty());
        console
            .claim(&agent(101, "builder"))
            .expect("every link is done");
        let view = console.view();
        assert_eq!(view.state, "claimed");
        assert!(!view.ready, "a claimed task is not ready: it is taken");
        assert_eq!(view.after, vec!["t1", "t2"], "the links stay as history");
    }

    #[test]
    fn a_link_that_fails_or_is_withdrawn_blocks_its_task_and_nothing_cascades() {
        let ledger = TaskLedger::new(EventBus::new());
        let flaky = create(&ledger, "flaky upstream");
        let dropped = create(&ledger, "dropped upstream");
        let pending = create(&ledger, "still to do");
        let after_flaky = budgeted(&ledger, "after the flaky one", 40);
        ledger
            .relink("t4", &["t1".to_string()])
            .expect("an open task relinks");
        let after_all =
            linked(&ledger, "after all three", &["t1", "t2", "t3"], None).expect("linked");

        run(&flaky, FinishOutcome::Failed);
        dropped.withdraw().expect("open -> withdrawn");

        let view = after_flaky.view();
        assert_eq!(view.state, "open", "a failed link fails nothing on its own");
        assert!(!view.ready);
        assert!(view.waiting_on.is_empty());
        assert_eq!(view.blocked_by, vec!["t1"]);
        assert_eq!(
            ledger.treasury().escrow_balance("t4"),
            40,
            "the blocked task keeps its escrow"
        );
        let refused = after_flaky
            .claim(&agent(101, "builder"))
            .expect_err("blocked");
        assert_eq!(
            refused.to_string(),
            "task t4 runs after t1, which ended without being done"
        );

        let view = after_all.view();
        assert_eq!(view.waiting_on, vec!["t3"]);
        assert_eq!(view.blocked_by, vec!["t1", "t2"]);
        assert_eq!(
            after_all
                .claim(&agent(101, "builder"))
                .unwrap_err()
                .to_string(),
            "task t5 runs after t3, which is not done yet, and t1 and t2, which ended without being done"
        );

        // The blocked task can still be withdrawn on purpose, or relinked and run.
        after_all.withdraw().expect("a blocked task is withdrawn");
        assert_eq!(after_all.view().blocked_by, Vec::<String>::new());
        ledger
            .relink("t4", &["t3".to_string()])
            .expect("relinked past the failed task");
        run(&pending, FinishOutcome::Done);
        assert!(after_flaky.is_ready());
    }

    #[test]
    fn links_are_held_to_the_contract_and_a_refused_create_hands_out_no_id() {
        let ledger = TaskLedger::new(EventBus::new());
        let free = create(&ledger, "in no project");
        let planned = linked(&ledger, "in pr1", &[], Some("pr1")).expect("a project's task");
        assert_eq!(planned.project(), Some("pr1"));
        assert_eq!(planned.view().project, Some("pr1"));
        assert_eq!(free.project(), None);

        let invalid = |after: &[&str], project: Option<&str>| {
            let refused = linked(&ledger, "refused", after, project).expect_err("refused");
            assert_eq!(refused.status(), 400, "{refused}");
            refused.to_string()
        };
        assert_eq!(
            invalid(&["t9"], None),
            "after names t9, which is not a task"
        );
        assert_eq!(invalid(&["t1", "t1"], None), "after names t1 twice");
        assert_eq!(
            invalid(&["t1"], Some("pr1")),
            "t1 is in no project, and this task is in project pr1: a task runs after tasks of its own project only"
        );
        assert_eq!(
            invalid(&["t2"], None),
            "t2 is in project pr1, and this task is in no project: a task runs after tasks of its own project only"
        );
        assert_eq!(
            invalid(&["t2"], Some("pr2")),
            "t2 is in project pr1, and this task is in project pr2: a task runs after tasks of its own project only"
        );
        assert_eq!(invalid(&[], Some("")), "project must not be empty");
        assert_eq!(
            invalid(&[], Some(&"p".repeat(MAX_PROJECT + 1))),
            format!("project must be at most {MAX_PROJECT} bytes")
        );
        let too_many: Vec<String> = (1..=MAX_AFTER + 1).map(|n| format!("t{n}")).collect();
        let too_many: Vec<&str> = too_many.iter().map(String::as_str).collect();
        assert_eq!(
            invalid(&too_many, None),
            format!(
                "after may name at most {MAX_AFTER} tasks, not {}",
                MAX_AFTER + 1
            )
        );
        assert_eq!(ledger.list().len(), 2, "no refused request created a task");

        // The same project's tasks link, and the next id is the next one.
        let next = linked(&ledger, "after pr1's first", &["t2"], Some("pr1")).expect("linked");
        assert_eq!(next.id(), "t3");
        assert_eq!(next.after(), vec!["t2"]);

        // MAX_AFTER links are inside the contract.
        let roots: Vec<String> = (0..MAX_AFTER)
            .map(|index| create(&ledger, &format!("root {index}")).id().to_string())
            .collect();
        let roots: Vec<&str> = roots.iter().map(String::as_str).collect();
        let wide = linked(&ledger, "after many", &roots, None).expect("the bound is inside");
        assert_eq!(wide.after().len(), MAX_AFTER);
    }

    #[test]
    fn a_task_outside_projects_moves_into_one_with_its_linked_group() {
        let ledger = TaskLedger::new(EventBus::new());
        create(&ledger, "design");
        linked(&ledger, "build", &["t1"], None).expect("linked");
        linked(&ledger, "test", &["t2"], None).expect("linked");
        create(&ledger, "unrelated");
        linked(&ledger, "elsewhere", &[], Some("pr1")).expect("in pr1");

        // Moving the middle task moves the whole chain, oldest first; the unrelated task stays outside.
        let moved = ledger.adopt("t2", "pr2").expect("moved");
        let ids: Vec<&str> = moved.iter().map(|task| task.id()).collect();
        assert_eq!(ids, ["t1", "t2", "t3"]);
        for task in &moved {
            assert_eq!(task.project(), Some("pr2"));
            assert_eq!(task.view().project, Some("pr2"));
        }
        assert_eq!(ledger.find("t4").expect("t4").project(), None);
        // The chain's links still hold, now inside pr2, and a new pr2 task may run after them.
        assert_eq!(ledger.find("t3").expect("t3").after(), vec!["t2"]);
        linked(&ledger, "ship", &["t3"], Some("pr2")).expect("a pr2 task runs after a moved one");

        // A task in a project never leaves it; unknown tasks and bad ids are refused.
        let refused = ledger.adopt("t1", "pr3").expect_err("in pr2 already");
        assert_eq!(
            refused,
            AdoptError::InProject {
                task: "t1".into(),
                project: "pr2".into()
            }
        );
        assert_eq!(refused.status(), 409);
        assert_eq!(ledger.adopt("t5", "pr2").expect_err("in pr1").status(), 409);
        assert_eq!(
            ledger.adopt("t99", "pr2").expect_err("no such task"),
            AdoptError::NotFound("t99".into())
        );
        assert_eq!(ledger.adopt("t4", " ").expect_err("blank").status(), 400);

        // The move is in the snapshot.
        let restored = TaskLedger::new(EventBus::new());
        restored.restore(&ledger.snapshot()).expect("restores");
        assert_eq!(restored.find("t2").expect("t2").project(), Some("pr2"));
        assert_eq!(restored.find("t4").expect("t4").project(), None);
    }

    #[test]
    fn a_relink_that_would_close_a_cycle_is_refused_with_the_cycle() {
        let ledger = TaskLedger::new(EventBus::new());
        create(&ledger, "design");
        linked(&ledger, "build", &["t1"], None).expect("linked");
        linked(&ledger, "test", &["t2"], None).expect("linked");
        let ids = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect::<Vec<_>>();

        let refused = ledger
            .relink("t1", &ids(&["t3"]))
            .expect_err("t1 cannot run after what runs after it");
        assert_eq!(refused, LinkError::Cycle(ids(&["t1", "t3", "t2", "t1"])));
        assert_eq!(refused.status(), 400);
        assert_eq!(
            refused.to_string(),
            "the links would close a cycle: t1 runs after t3, which runs after t2, which runs after t1"
        );
        let refused = ledger
            .relink("t1", &ids(&["t1"]))
            .expect_err("a task cannot run after itself");
        assert_eq!(refused, LinkError::Cycle(ids(&["t1", "t1"])));
        assert_eq!(
            refused.to_string(),
            "the links would close a cycle: t1 runs after t1"
        );
        let refused = ledger
            .relink("t2", &ids(&["t3"]))
            .expect_err("a two-task cycle");
        assert_eq!(refused, LinkError::Cycle(ids(&["t2", "t3", "t2"])));
        assert!(
            ledger.find("t1").expect("t1").after().is_empty(),
            "a refused relink changes nothing"
        );
        assert_eq!(ledger.find("t2").expect("t2").after(), vec!["t1"]);

        // The shortest cycle is the one reported.
        linked(&ledger, "ship", &["t1", "t3"], None).expect("linked");
        assert_eq!(
            ledger.relink("t1", &ids(&["t4"])).unwrap_err(),
            LinkError::Cycle(ids(&["t1", "t4", "t1"]))
        );

        // A relink that closes no cycle replaces the links whole.
        let test = ledger.relink("t3", &ids(&["t1"])).expect("no cycle");
        assert_eq!(test.after(), vec!["t1"]);
        ledger
            .relink("t2", &ids(&["t3"]))
            .expect("t3 no longer runs after t2");
        assert_eq!(ledger.find("t2").expect("t2").view().waiting_on, vec!["t3"]);
        assert!(
            ledger
                .relink("t3", &[])
                .expect("unlinked")
                .after()
                .is_empty()
        );
    }

    #[test]
    fn only_an_open_tasks_links_change() {
        let ledger = TaskLedger::new(EventBus::new());
        let first = create(&ledger, "first");
        let second = create(&ledger, "second");
        linked(&ledger, "planned", &[], Some("pr1")).expect("a project's task");
        second.claim(&agent(100, "builder")).expect("claim");

        let refused = ledger
            .relink("t2", &["t1".to_string()])
            .expect_err("a claimed task keeps its links");
        assert_eq!(
            refused,
            LinkError::NotOpen {
                task: "t2".to_string(),
                state: TaskState::Claimed
            }
        );
        assert_eq!(refused.status(), 409);
        assert_eq!(
            refused.to_string(),
            "task t2 is claimed: only an open task's links can change"
        );

        let refused = ledger.relink("t9", &[]).expect_err("no such task");
        assert_eq!(refused, LinkError::NotFound("t9".to_string()));
        assert_eq!(refused.status(), 404);

        for (id, after) in [
            ("t1", vec!["t9".to_string()]),
            ("t1", vec!["t2".to_string(), "t2".to_string()]),
            ("t3", vec!["t1".to_string()]),
        ] {
            let refused = ledger.relink(id, &after).expect_err("outside the contract");
            assert!(matches!(refused, LinkError::Invalid(_)), "{refused}");
            assert_eq!(refused.status(), 400);
        }
        assert!(first.after().is_empty());
    }

    #[test]
    fn a_pass_that_claims_ready_tasks_claims_them_in_link_order() {
        // The scheduler's pass (apid's `assign_tasks`) over a diamond: design,
        // then build and docs side by side, then the release after both.
        let ledger = TaskLedger::new(EventBus::new());
        create(&ledger, "design");
        linked(&ledger, "build", &["t1"], None).expect("linked");
        linked(&ledger, "docs", &["t1"], None).expect("linked");
        linked(&ledger, "release", &["t2", "t3"], None).expect("linked");
        let ready = |ledger: &TaskLedger| -> Vec<String> {
            ledger
                .list()
                .iter()
                .filter(|task| task.is_ready())
                .map(|task| task.id().to_string())
                .collect()
        };

        assert_eq!(ready(&ledger), vec!["t1"]);
        let mut order = Vec::new();
        // One idle worker: each pass claims the first ready task, which is done before the next pass.
        while let Some(task) = ledger.list().into_iter().find(|task| task.is_ready()) {
            for upstream in task.after() {
                assert_eq!(
                    ledger.find(&upstream).expect("a link").state(),
                    "done",
                    "{} is claimed before {upstream} is done",
                    task.id()
                );
            }
            order.push(task.id().to_string());
            run(&task, FinishOutcome::Done);
            if order == ["t1"] {
                assert_eq!(
                    ready(&ledger),
                    vec!["t2", "t3"],
                    "both sides are ready at once"
                );
            }
            if order == ["t1", "t2"] {
                assert_eq!(ready(&ledger), vec!["t3"], "the release waits on the docs");
                assert_eq!(ledger.find("t4").expect("t4").view().waiting_on, vec!["t3"]);
            }
        }
        assert_eq!(order, vec!["t1", "t2", "t3", "t4"]);
        assert!(ledger.list().iter().all(|task| task.state() == "done"));
    }

    #[test]
    fn links_survive_a_snapshot_and_link_the_restored_tasks() {
        let events = EventBus::new();
        let ledger = TaskLedger::new(Arc::clone(&events));
        linked(&ledger, "plan", &[], Some("pr1")).expect("a project's task");
        linked(&ledger, "build", &["t1"], Some("pr1")).expect("linked");
        create(&ledger, "unrelated");
        run(&ledger.find("t3").expect("t3"), FinishOutcome::Done);
        let before = views(&ledger);
        assert_eq!(before[1]["after"], serde_json::json!(["t1"]));
        assert_eq!(before[1]["project"], "pr1");
        assert_eq!(before[1]["waiting_on"], serde_json::json!(["t1"]));

        let restored =
            TaskLedger::with_treasury(Arc::clone(&events), Arc::clone(ledger.treasury()));
        restored.restore(&ledger.snapshot()).expect("restores");
        assert_eq!(views(&restored), before);

        // The restored link holds the restored task, not the old ledger's.
        run(&restored.find("t1").expect("t1"), FinishOutcome::Done);
        assert!(restored.find("t2").expect("t2").is_ready());
        assert!(
            !ledger.find("t2").expect("t2").is_ready(),
            "the old ledger is untouched"
        );
    }

    #[test]
    fn a_snapshot_from_before_links_loads_with_none() {
        let ledger = TaskLedger::new(EventBus::new());
        create(&ledger, "old work");
        let mut value: serde_json::Value =
            serde_json::from_str(&ledger.snapshot()).expect("the snapshot is JSON");
        let record = value["tasks"][0].as_object_mut().expect("a record");
        record.remove("after");
        record.remove("project");

        let restored = TaskLedger::new(EventBus::new());
        restored
            .restore(&value.to_string())
            .expect("an old record restores");
        let task = restored.find("t1").expect("t1");
        let view = task.view();
        assert!(view.after.is_empty());
        assert_eq!(view.project, None);
        assert!(view.ready);
    }

    #[test]
    fn a_snapshot_whose_links_break_the_rules_is_refused() {
        let ledger = TaskLedger::new(EventBus::new());
        create(&ledger, "first");
        linked(&ledger, "second", &["t1"], None).expect("linked");
        linked(&ledger, "third", &[], Some("pr1")).expect("a project's task");
        let before = views(&ledger);
        let value: serde_json::Value =
            serde_json::from_str(&ledger.snapshot()).expect("the snapshot is JSON");
        let with = |task: usize, after: serde_json::Value| {
            let mut changed = value.clone();
            changed["tasks"][task]["after"] = after;
            changed.to_string()
        };

        for (garbage, refusal) in [
            (
                with(1, serde_json::json!(["t9"])),
                "which the snapshot does not hold",
            ),
            (
                with(1, serde_json::json!(["t3"])),
                "which is in another project",
            ),
            (
                with(1, serde_json::json!(["t1", "t1"])),
                "after names t1 twice",
            ),
            (with(0, serde_json::json!(["t2"])), "close a cycle"),
            (with(1, serde_json::json!(["t2"])), "close a cycle"),
        ] {
            let refused = ledger.restore(&garbage).expect_err("refused");
            assert!(refused.contains(refusal), "{refused}");
            assert_eq!(views(&ledger), before, "a refused restore changes nothing");
        }
    }

    /// The task events a subscriber holds, as (id, state, ready), oldest first.
    fn announced(receiver: &std::sync::mpsc::Receiver<Event>) -> Vec<(String, &'static str, bool)> {
        receiver
            .try_iter()
            .filter_map(|event| match event {
                Event::Task {
                    id, state, ready, ..
                } => Some((id, state, ready)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_task_that_ends_announces_the_open_tasks_that_run_after_it() {
        let events = EventBus::new();
        let receiver = events.subscribe();
        let ledger = TaskLedger::new(Arc::clone(&events));
        let base = create(&ledger, "base");
        linked(&ledger, "next", &["t1"], None).expect("linked");
        let dropped = linked(&ledger, "dropped", &["t1"], None).expect("linked");
        dropped.withdraw().expect("open -> withdrawn");
        assert_eq!(
            announced(&receiver),
            vec![
                ("t1".to_string(), "open", true),
                ("t2".to_string(), "open", false),
                ("t3".to_string(), "open", false),
                ("t3".to_string(), "withdrawn", false),
            ],
            "a task is announced as it is created, not ready while it waits"
        );

        run(&base, FinishOutcome::Done);
        assert_eq!(
            announced(&receiver),
            vec![
                ("t1".to_string(), "claimed", false),
                ("t1".to_string(), "done", false),
                ("t2".to_string(), "open", true),
            ],
            "the open task after it is announced ready, and the withdrawn one is not announced"
        );

        // A link that fails announces its task too: still open, blocked.
        let flaky = create(&ledger, "flaky");
        linked(&ledger, "after the flaky one", &["t4"], None).expect("linked");
        run(&flaky, FinishOutcome::Failed);
        assert_eq!(
            announced(&receiver).last(),
            Some(&("t5".to_string(), "open", false))
        );
        assert_eq!(ledger.find("t5").expect("t5").view().blocked_by, vec!["t4"]);
    }

    #[test]
    fn a_relink_publishes_the_new_links_and_moves_the_announcements() {
        let events = EventBus::new();
        let ledger = TaskLedger::new(Arc::clone(&events));
        let first = create(&ledger, "first");
        let second = create(&ledger, "second");
        linked(&ledger, "third", &["t1"], None).expect("linked");
        let receiver = events.subscribe();

        ledger
            .relink("t3", &["t2".to_string()])
            .expect("an open task relinks");
        let frames: Vec<String> = receiver.try_iter().map(|event| event.frame()).collect();
        assert_eq!(
            frames,
            vec![
                "event: task_link\ndata: {\"task\":\"t3\",\"after\":[\"t2\"]}\n\n",
                "event: task\ndata: {\"id\":\"t3\",\"state\":\"open\",\"assignee\":null,\"agent_cid\":null,\"project\":null,\"ready\":false}\n\n",
            ]
        );

        // t1 no longer announces t3 when it ends; t2 does.
        run(&first, FinishOutcome::Done);
        assert!(
            announced(&receiver).iter().all(|(id, _, _)| id == "t1"),
            "t3 no longer runs after t1"
        );
        run(&second, FinishOutcome::Done);
        assert_eq!(
            announced(&receiver).last(),
            Some(&("t3".to_string(), "open", true))
        );
    }

    #[test]
    fn a_restored_task_announces_the_tasks_that_run_after_it() {
        let events = EventBus::new();
        let ledger = TaskLedger::new(Arc::clone(&events));
        create(&ledger, "first");
        linked(&ledger, "second", &["t1"], None).expect("linked");
        let restored =
            TaskLedger::with_treasury(Arc::clone(&events), Arc::clone(ledger.treasury()));
        restored.restore(&ledger.snapshot()).expect("restores");
        let receiver = events.subscribe();

        run(&restored.find("t1").expect("t1"), FinishOutcome::Done);
        assert_eq!(
            announced(&receiver).last(),
            Some(&("t2".to_string(), "open", true))
        );
    }

    /// One task of a batch in project `project`, running after `after` (existing ids) and
    /// `after_batch` (earlier tasks of the batch).
    fn item(goal: &str, budget: Option<u64>, after: &[&str], after_batch: &[usize]) -> BatchTask {
        BatchTask {
            request: CreateTask {
                goal: goal.to_string(),
                acceptance: None,
                budget,
                role: None,
                after: after.iter().map(|id| id.to_string()).collect(),
                project: Some("pr1".to_string()),
            },
            after_batch: after_batch.to_vec(),
        }
    }

    #[test]
    fn a_batch_is_created_whole_with_its_links() {
        let events = EventBus::new();
        let ledger = TaskLedger::new(Arc::clone(&events));
        linked(&ledger, "already there", &[], Some("pr1")).expect("an existing task");
        let receiver = events.subscribe();

        let created = ledger
            .create_all(&[
                item("design", Some(300), &[], &[]),
                item("build", Some(300), &[], &[0]),
                item("ship", None, &["t1"], &[0, 1]),
            ])
            .expect("a batch inside the contract");
        let ids: Vec<&str> = created.iter().map(|task| task.id()).collect();
        assert_eq!(ids, vec!["t2", "t3", "t4"]);
        assert_eq!(created[1].after(), vec!["t2"]);
        assert_eq!(created[2].after(), vec!["t1", "t2", "t3"]);
        assert!(created[0].is_ready());
        assert!(!created[1].is_ready() && !created[2].is_ready());
        assert_eq!(ledger.treasury().escrow_balance("t2"), 300);
        assert_eq!(
            announced(&receiver),
            vec![
                ("t2".to_string(), "open", true),
                ("t3".to_string(), "open", false),
                ("t4".to_string(), "open", false),
            ],
            "the batch is announced once it is all there, in order"
        );
        // The links inside the batch announce like any other.
        run(&created[0], FinishOutcome::Done);
        assert!(created[1].is_ready());
        assert_eq!(created[2].view().waiting_on, vec!["t1", "t3"]);
    }

    #[test]
    fn a_batch_outside_the_contract_creates_nothing() {
        let ledger = TaskLedger::new(EventBus::new());
        linked(&ledger, "elsewhere", &[], Some("pr2")).expect("another project's task");
        let refused = |batch: &[BatchTask]| {
            let error = ledger.create_all(batch).expect_err("refused");
            assert_eq!(error.status(), 400, "{error}");
            error.to_string()
        };
        assert_eq!(
            refused(&[item("a", None, &[], &[1]), item("b", None, &[], &[])]),
            "task 1 of the batch: it may run after an earlier task of the batch only, not task 2"
        );
        assert_eq!(
            refused(&[item("a", None, &[], &[]), item("b", None, &[], &[0, 0])]),
            "task 2 of the batch: it names task 1 of the batch twice"
        );
        let mut other = item("b", None, &[], &[0]);
        other.request.project = Some("pr3".to_string());
        assert_eq!(
            refused(&[item("a", None, &[], &[]), other]),
            "task 2 of the batch: task 1 of the batch is in project pr1, and this task is in project pr3: a task runs after tasks of its own project only"
        );
        assert_eq!(
            refused(&[item("a", None, &["t1"], &[])]),
            "task 1 of the batch: t1 is in project pr2, and this task is in project pr1: a task runs after tasks of its own project only"
        );
        assert_eq!(
            refused(&[item("", None, &[], &[])]),
            "task 1 of the batch: goal must not be empty"
        );
        assert_eq!(ledger.list().len(), 1, "no refused batch created a task");
        assert_eq!(create(&ledger, "next").id(), "t2", "and none took an id");
    }

    #[test]
    fn a_batch_the_treasury_cannot_fund_is_given_back_whole() {
        let ledger = TaskLedger::new(EventBus::new());
        // Leave room for one more account: the first escrow takes it, the second is refused.
        for index in 0..crate::treasury::MAX_ACCOUNTS - 3 {
            ledger
                .treasury()
                .open_account(
                    crate::treasury::AccountKind::TownBudget,
                    &format!("town{index}"),
                    0,
                )
                .expect("an account under the cap");
        }
        let budget_before = ledger.treasury().plaza_budget_balance();
        let refused = ledger
            .create_all(&[
                item("first", Some(300), &[], &[]),
                item("second", Some(300), &[], &[0]),
            ])
            .expect_err("the second escrow cannot be opened");
        assert!(
            matches!(
                refused,
                CreateError::Treasury(PostError::AccountsFull { .. })
            ),
            "{refused:?}"
        );
        assert!(ledger.list().is_empty(), "no task of the batch was created");
        assert_eq!(
            ledger.treasury().escrow_balance("t1"),
            0,
            "the first mint went back"
        );
        assert_eq!(ledger.treasury().plaza_budget_balance(), budget_before);
        // t1's escrow keeps its mint and its refund, so its id is not handed out again.
        assert_eq!(create(&ledger, "free").id(), "t2");
    }
}
