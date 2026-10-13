//! Work items: per-agent FIFO queues (one item in flight per agent), sending
//! them through the hub, closing them, and the scheduler that turns open
//! tasks, done tasks awaiting review and open motions into work.
//!
//! OWNER: slice "scheduler". Signatures used by other modules are marked
//! `// contract` — keep them.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use apz_plaza::council::is_seated;
use apz_plaza::tasks::{FinishOutcome, TaskState};
use apz_plaza::treasury::{PostError, MARGIN_CAP_PCT};
use apz_proto::wire::{HostMessage, WorkKind, WorkStatus};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agents::{connected_agent, Agent, Status};
use crate::approvals::{Approval, State as ApprovalState, Store as ApprovalStore};
use crate::settings::{tier_of, Tier};
use crate::state::App;

/// How many council motions the scheduler looks at per tick.
const MOTION_SCAN_LIMIT: usize = 50;

/// The role that verifies done tasks before they pay.
pub const REVIEWER_ROLE: &str = "reviewer";

/// What a work item is about, beyond its prompt.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Origin {
    pub task: Option<String>,
    pub motion: Option<String>,
    /// For `message` work: the agent who wrote.
    pub from_cid: Option<u64>,
    /// True for the owner's own chat (the owner chat route sets it); automated chats leave it false.
    #[serde(default)]
    pub owner: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItem {
    pub id: u64,
    pub cid: u64,
    pub kind: WorkKind,
    pub thread: String,
    pub prompt: String,
    pub origin: Origin,
    pub created_ms: u64,
    pub started_ms: Option<u64>,
}

/// One agent's queue: the item in flight and the items waiting behind it.
#[derive(Default, Clone, Serialize, Deserialize)]
struct Lane {
    queue: VecDeque<WorkItem>,
    current: Option<WorkItem>,
}

/// The queue logic, free of the app so it can be tested on its own.
#[derive(Default, Clone)]
struct Lanes {
    lanes: BTreeMap<u64, Lane>,
}

impl Lanes {
    fn push(&mut self, item: WorkItem) {
        self.lanes.entry(item.cid).or_default().queue.push_back(item);
    }

    /// Moves the next waiting item in flight, unless one already is.
    fn start_next(&mut self, cid: u64, now: u64) -> Option<WorkItem> {
        let lane = self.lanes.get_mut(&cid)?;
        if lane.current.is_some() {
            return None;
        }
        let mut item = lane.queue.pop_front()?;
        item.started_ms = Some(now);
        lane.current = Some(item.clone());
        Some(item)
    }

    /// Closes item `work` if it is the one in flight for `cid`.
    fn finish(&mut self, cid: u64, work: u64) -> Option<WorkItem> {
        let lane = self.lanes.get_mut(&cid)?;
        if lane.current.as_ref().map(|c| c.id) != Some(work) {
            return None;
        }
        lane.current.take()
    }

    /// Puts an item that failed to send back at the front, not in flight.
    fn requeue_front(&mut self, mut item: WorkItem) {
        item.started_ms = None;
        let lane = self.lanes.entry(item.cid).or_default();
        if lane.current.as_ref().map(|c| c.id) == Some(item.id) {
            lane.current = None;
        }
        lane.queue.push_front(item);
    }

    fn current(&self, cid: u64) -> Option<WorkItem> {
        self.lanes.get(&cid)?.current.clone()
    }

    fn queued(&self, cid: u64) -> Vec<WorkItem> {
        self.lanes.get(&cid).map(|l| l.queue.iter().cloned().collect()).unwrap_or_default()
    }

    /// Removes everything for `cid`: the item in flight first, then the queue.
    fn drain(&mut self, cid: u64) -> Vec<WorkItem> {
        let Some(lane) = self.lanes.remove(&cid) else {
            return Vec::new();
        };
        lane.current.into_iter().chain(lane.queue).collect()
    }

    /// Whether an item for plaza task `task` is in flight or waiting.
    fn holds_task(&self, task: &str) -> bool {
        self.lanes.values().any(|lane| {
            lane.current
                .iter()
                .chain(lane.queue.iter())
                .any(|item| item.origin.task.as_deref() == Some(task))
        })
    }
}

/// The work queues: one lane per agent, one item in flight per lane.
/// A done task awaiting its reviewer's verdict.
struct Review {
    /// The worker whose done is being verified (paid on a pass).
    worker: u64,
    /// The reviewer's work item: only its verdict, or its end, settles the task.
    work: u64,
}

pub struct Queues {
    lanes: Mutex<Lanes>,
    next_id: AtomicU64,
    /// task id → the agent that posted it (the orchestrator), told when it ends.
    posters: Mutex<std::collections::HashMap<String, u64>>,
    /// task id → the review awaiting a verdict.
    reviews: Mutex<std::collections::HashMap<String, Review>>,
    /// Held while a task is reviewed or ruled on, so a task is paid at most once and no review
    /// starts on a task that a ruling has already ended.
    rulings: Mutex<()>,
    /// task id → credits metered against it so far (its real cost).
    spend: Mutex<std::collections::HashMap<String, i64>>,
}

impl Default for Queues {
    fn default() -> Self {
        Self {
            lanes: Mutex::new(Lanes::default()),
            next_id: AtomicU64::new(1),
            posters: Mutex::default(),
            reviews: Mutex::default(),
            rulings: Mutex::default(),
            spend: Mutex::default(),
        }
    }
}

impl Queues {
    /// The lock, recovered if a previous holder panicked.
    fn locked(&self) -> MutexGuard<'_, Lanes> {
        self.lanes.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Queues work for `cid` and sends it if the agent is idle; the work id.
    pub fn enqueue(&self, app: &App, cid: u64, kind: WorkKind, thread: String, prompt: String, origin: Origin) -> u64 {
        // contract
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let item = WorkItem {
            id,
            cid,
            kind,
            thread,
            prompt,
            origin,
            created_ms: apz_plaza::now_ms(),
            started_ms: None,
        };
        self.locked().push(item);
        self.dispatch(app, cid);
        id
    }

    /// The item in flight for `cid`.
    pub fn current(&self, cid: u64) -> Option<WorkItem> {
        // contract
        self.locked().current(cid)
    }

    /// Items waiting behind the current one.
    pub fn queued(&self, cid: u64) -> Vec<WorkItem> {
        // contract
        self.locked().queued(cid)
    }

    /// The agent reported `work_done`: close the item (task/chat/message
    /// effects), then send the next one.
    pub fn complete(&self, app: &App, cid: u64, work: u64, status: WorkStatus, summary: &str) {
        // contract
        let Some(item) = self.locked().finish(cid, work) else {
            return;
        };
        let state = status_str(status);
        app.transcripts.record(&app.bus, cid, work, "work_end", json!({ "status": state, "summary": summary }));
        app.bus.publish("work", &json!({ "cid": cid, "work": work, "state": "ended", "status": state }));
        if let Some(task) = item.origin.task.as_deref() {
            // A worker's done is a claim: the task waits for a reviewer's verdict, and its
            // escrow stays held until then (see `assign_reviews`).
            let waiting = matches!(status, WorkStatus::Done)
                && matches!(item.kind, WorkKind::Task)
                && self.await_review(app, task, summary);
            if !waiting {
                settle_task(app, task, status, summary);
                self.tell_poster(app, task, cid, state, summary);
            }
        }
        self.review_lost(app, item.id, "reviewer ended without a verdict");
        app.agents.update(cid, |a| a.status = Status::Idle);
        self.dispatch(app, cid);
        // This agent is free now: it may be the reviewer a waiting task was waiting for.
        self.assign_reviews(app);
    }

    /// Records that `cid` posted `task`, so it hears when the task ends.
    pub fn note_poster(&self, task: &str, cid: u64) {
        self.posters.lock().unwrap_or_else(|p| p.into_inner()).insert(task.to_string(), cid);
    }

    /// Tells a task's poster (the orchestrator) that it ended, continuing the
    /// poster's conversation with the operator so it keeps its plan.
    fn tell_poster(&self, app: &App, task: &str, worker: u64, state: &str, summary: &str) {
        let poster = self.posters.lock().unwrap_or_else(|p| p.into_inner()).remove(task);
        let Some(poster) = poster.filter(|&p| p != worker) else { return };
        crate::society::task_ended(app, task, poster, worker, state == "done");
        let worker_name = app.agents.get(worker).map_or_else(|| format!("agent {worker}"), |a| a.name);
        let prompt = format!(
            "[plaza] Task {task}, worked by {worker_name}, ended: {state}.\nSummary: {summary}\n\n\
             Review it (plaza_task_result), continue your plan, and tell the operator when the goal is complete."
        );
        self.enqueue(app, poster, WorkKind::Chat, format!("op-{poster}"), prompt, Origin::default());
    }

    /// The operator cancels the current item (sends `cancel`).
    pub fn cancel(&self, app: &App, cid: u64, reason: &str) -> bool {
        // contract
        let Some(item) = self.locked().current(cid) else {
            return false;
        };
        let message = HostMessage::Cancel { work: item.id, reason: reason.to_string() };
        matches!(
            crate::fleet::send(app, cid, &json!({ "message": message })),
            Ok((code, _)) if (200..300).contains(&code)
        )
    }

    /// Drops the agent's queued items, not the one in flight; returns how many.
    pub fn clear(&self, cid: u64) -> usize {
        let mut lanes = self.locked();
        let Some(lane) = lanes.lanes.get_mut(&cid) else {
            return 0;
        };
        let dropped = lane.queue.len();
        lane.queue.clear();
        dropped
    }

    /// The agent went offline: fail its items (tasks return to open).
    pub fn agent_gone(&self, app: &App, cid: u64) {
        // contract
        let drained = self.locked().drain(cid);
        for item in drained {
            app.transcripts.record(
                &app.bus,
                cid,
                item.id,
                "work_end",
                json!({ "status": "failed", "summary": "agent went offline" }),
            );
            app.bus.publish("work", &json!({ "cid": cid, "work": item.id, "state": "ended", "status": "failed" }));
            if let Some(task) = item.origin.task.as_deref() {
                settle_task(app, task, WorkStatus::Failed, "agent went offline");
            }
            self.review_lost(app, item.id, "reviewer went offline");
        }
        app.agents.update(cid, |a| a.status = Status::Offline);
    }

    /// Adds metered `credits` to `task`'s real cost.
    pub fn add_spend(&self, task: &str, credits: i64) {
        // contract
        let mut spend = self.spend.lock().unwrap_or_else(|p| p.into_inner());
        *spend.entry(task.to_string()).or_insert(0) += credits;
    }

    /// The credits metered against `task` so far.
    fn spent(&self, task: &str) -> i64 {
        self.spend.lock().unwrap_or_else(|p| p.into_inner()).get(task).copied().unwrap_or(0)
    }

    /// The worker reported `task` done: it waits for a reviewer's verdict, and its escrow stays
    /// held. False when the task cannot wait (it already ended), and the caller settles it as before.
    fn await_review(&self, app: &App, task: &str, summary: &str) -> bool {
        app.plaza
            .tasks
            .find(task)
            .is_some_and(|record| record.await_review(Some(summary.to_string())).is_ok())
    }

    /// Gives each task awaiting review, with no reviewer on it yet, to the next idle reviewer. A task
    /// that has waited past the review timeout with none gets one owner approval (kind `review`).
    pub fn assign_reviews(&self, app: &App) {
        let timeout_ms = u64::from(app.settings.get().review_timeout_minutes) * 60_000;
        let now = apz_plaza::now_ms();
        for task in app.plaza.tasks.list() {
            let view = task.view();
            if view.state != TaskState::AwaitingReview.as_str() {
                continue;
            }
            let Some(worker) = view.agent_cid else {
                continue;
            };
            let id = view.id.to_string();
            let summary = view.note.clone().unwrap_or_default();
            let reviewed = self.reviews.lock().unwrap_or_else(|p| p.into_inner()).contains_key(&id);
            if reviewed || self.start_review(app, &id, worker, &summary) {
                continue;
            }
            let waited = now.saturating_sub(view.updated_at);
            if review_overdue(waited, timeout_ms) {
                let worker_name = app.agents.get(worker).map_or_else(|| format!("agent {worker}"), |a| a.name);
                let escalation =
                    escalate_review(&app.approvals, &id, worker, &worker_name, view.goal, &summary, waited);
                if let Some(item) = escalation {
                    app.bus.publish("approval", &item);
                }
            }
        }
    }

    /// Hands a done task to the next idle reviewer; false when none is connected or the task is no
    /// longer awaiting review. Runs under `rulings`, so a ruling cannot land between the check and the
    /// review's start.
    fn start_review(&self, app: &App, task: &str, worker: u64, summary: &str) -> bool {
        let _rulings = self.rulings.lock().unwrap_or_else(|p| p.into_inner());
        let Some(record) = app.plaza.tasks.find(task) else {
            return false;
        };
        if record.state() != TaskState::AwaitingReview.as_str() {
            return false;
        }
        let Some(reviewer) = next_reviewer(&app.agents.list(), worker) else {
            return false;
        };
        let worker_name = app.agents.get(worker).map_or_else(|| format!("agent {worker}"), |a| a.name);
        let acceptance = record.acceptance().map_or_else(|| "none stated".to_string(), |a| a.to_string());
        let prompt = format!(
            "Verify task {task}: {goal}\nAcceptance: {acceptance}\nThe worker ({worker_name}) reported: {summary}\n\
             Check it (use plaza_get_file for shared files) and call plaza_verdict with pass true/false and short notes.",
            goal = record.goal(),
        );
        let work = self.enqueue(app, reviewer, WorkKind::Task, format!("review-{task}"), prompt, Origin::default());
        self.reviews.lock().unwrap_or_else(|p| p.into_inner()).insert(task.to_string(), Review { worker, work });
        true
    }

    /// The reviewer's verdict on `task`, from review work `work`. Pass pays the
    /// worker its margin-capped share and completes the task; fail fails it (the
    /// escrow goes back to the plaza's budget).
    pub fn verdict(&self, app: &App, task: &str, work: u64, pass: bool, notes: &str) -> Result<String, String> {
        // contract
        let rulings = self.rulings.lock().unwrap_or_else(|p| p.into_inner());
        let mut reviews = self.reviews.lock().unwrap_or_else(|p| p.into_inner());
        if !reviews.get(task).is_some_and(|r| r.work == work) {
            return Err(format!("no verdict is pending on task {task} from this review"));
        }
        let review = reviews.remove(task).ok_or_else(|| format!("no verdict is pending on task {task}"))?;
        drop(reviews);
        let summary = self.settle_ruling(app, task, review.worker, work, pass, notes);
        // The owner's card for this task, if one was raised, is no longer needed.
        publish_withdrawn(app, &withdraw_pending_review(&app.approvals, task, RULED_FIRST));
        drop(rulings);
        self.tell_poster(app, task, review.worker, if pass { "done" } else { "failed" }, notes);
        Ok(summary)
    }

    /// The owner's ruling on `task`, which is awaiting review: a pass pays the worker and completes the
    /// task, a fail fails it. A reviewer still working on it is no longer awaited.
    pub fn owner_verdict(&self, app: &App, task: &str, pass: bool, notes: &str) -> Result<String, String> {
        let rulings = self.rulings.lock().unwrap_or_else(|p| p.into_inner());
        let record = app.plaza.tasks.find(task).ok_or_else(|| format!("no task {task}"))?;
        let view = record.view();
        if view.state != TaskState::AwaitingReview.as_str() {
            return Err(format!("task {task} is {}; it is not awaiting review", view.state));
        }
        let worker = view.agent_cid.ok_or_else(|| format!("task {task} names no worker"))?;
        let reviewer = self.reviews.lock().unwrap_or_else(|p| p.into_inner()).remove(task);
        let work = reviewer.map_or(0, |r| r.work);
        let summary = self.settle_ruling(app, task, worker, work, pass, notes);
        drop(rulings);
        self.tell_poster(app, task, worker, if pass { "done" } else { "failed" }, notes);
        Ok(summary)
    }

    /// Pays the worker for a pass and ends the task, which releases the rest of its escrow. The caller
    /// holds `rulings`, so the task is paid at most once.
    fn settle_ruling(&self, app: &App, task: &str, worker: u64, work: u64, pass: bool, notes: &str) -> String {
        let paid = if pass { self.pay_worker(app, task, worker, work) } else { 0 };
        let status = if pass { WorkStatus::Done } else { WorkStatus::Failed };
        settle_task(app, task, status, notes);
        format!("verdict {}: paid {paid}", if pass { "pass" } else { "fail" })
    }

    /// Ends a review whose reviewer ended, or went offline, without a verdict: the task fails.
    fn review_lost(&self, app: &App, work: u64, reason: &str) {
        let rulings = self.rulings.lock().unwrap_or_else(|p| p.into_inner());
        let mut reviews = self.reviews.lock().unwrap_or_else(|p| p.into_inner());
        let found = reviews.iter().find(|(_, r)| r.work == work).map(|(task, _)| task.clone());
        let Some(task) = found else {
            return;
        };
        let Some(review) = reviews.remove(&task) else {
            return;
        };
        drop(reviews);
        settle_task(app, &task, WorkStatus::Failed, reason);
        drop(rulings);
        self.tell_poster(app, &task, review.worker, "failed", reason);
    }

    /// Pays `worker` 20% of `task`'s real cost, capped by the chain's margin
    /// cap; returns the amount paid.
    fn pay_worker(&self, app: &App, task: &str, worker: u64, work: u64) -> i64 {
        let spent = self.spent(task);
        let treasury = &app.plaza.treasury;
        let cap = spent * MARGIN_CAP_PCT / 100;
        let paid = if cap <= 0 {
            0
        } else {
            match treasury.pay_agent(task, worker, cap) {
                Ok(_) => cap,
                // The margin cap refused: pay what it still allows, or nothing.
                Err(PostError::MarginCap { paid_to_agents, real_cost, .. }) => {
                    let allowance = real_cost * i128::from(MARGIN_CAP_PCT) / 100 - paid_to_agents;
                    match i64::try_from(allowance) {
                        Ok(allowance) if allowance > 0 && treasury.pay_agent(task, worker, allowance).is_ok() => {
                            allowance
                        }
                        _ => 0,
                    }
                }
                Err(_) => 0,
            }
        };
        app.transcripts.record(
            &app.bus,
            worker,
            work,
            "payment",
            json!({ "task": task, "amount": paid, "spent": spent }),
        );
        paid
    }

    /// Starts the next item for every idle, connected agent that has queued work and nothing
    /// in flight — after a restart (restored queues) or anything else that left work waiting.
    pub fn dispatch_idle(&self, app: &App) {
        for agent in app.agents.list() {
            if agent.status == Status::Idle && self.current(agent.cid).is_none() && !self.queued(agent.cid).is_empty() {
                self.dispatch(app, agent.cid);
            }
        }
    }

    /// Sends the next waiting item to `cid` if it is reachable and idle.
    fn dispatch(&self, app: &App, cid: u64) {
        // Held by STOP ALL: queued work waits until the owner resumes.
        if app.control.paused() || app.agents.get(cid).is_none_or(|a| a.status == Status::Offline) {
            return;
        }
        let Some(item) = self.locked().start_next(cid, apz_plaza::now_ms()) else {
            return;
        };
        app.agents.update(cid, |a| a.status = Status::Working);
        let message = HostMessage::Work {
            work: item.id,
            kind: item.kind,
            thread: item.thread.clone(),
            prompt: item.prompt.clone(),
        };
        let sent = crate::fleet::send(app, cid, &json!({ "message": message }));
        if !matches!(sent, Ok((code, _)) if (200..300).contains(&code)) {
            self.locked().requeue_front(item);
            app.agents.update(cid, |a| a.status = Status::Idle);
            return;
        }
        app.transcripts.record(
            &app.bus,
            cid,
            item.id,
            "work_start",
            json!({
                "kind": item.kind,
                "thread": item.thread,
                "prompt": item.prompt,
                "origin": item.origin,
            }),
        );
        app.bus.publish("work", &json!({ "cid": cid, "work": item.id, "state": "started" }));
    }
}

/// The wire spelling of a work status, for the transcript and events.
fn status_str(status: WorkStatus) -> &'static str {
    match status {
        WorkStatus::Done => "done",
        WorkStatus::Failed => "failed",
        WorkStatus::Stopped => "stopped",
    }
}

/// Closes a task from its work: done stays done, anything else fails it.
fn settle_task(app: &App, task: &str, status: WorkStatus, summary: &str) {
    let Some(task) = app.plaza.tasks.find(task) else {
        return;
    };
    let outcome = match status {
        WorkStatus::Done => FinishOutcome::Done,
        WorkStatus::Failed | WorkStatus::Stopped => FinishOutcome::Failed,
    };
    // A task that already ended refuses the finish; that is not an error here.
    let _ = task.finish(outcome, Some(summary.to_string()));
}

/// The kind of owner approval that rules on a done task no reviewer took in time.
pub const REVIEW_KIND: &str = "review";

/// The idle reviewer a done task goes to next: the first idle agent of the reviewer role that is not
/// the worker.
fn next_reviewer(agents: &[Agent], worker: u64) -> Option<u64> {
    agents
        .iter()
        .find(|a| a.cid != worker && a.role == REVIEWER_ROLE && a.status == Status::Idle)
        .map(|a| a.cid)
}

/// Whether a task that has waited `waited_ms` for a reviewer has waited past `timeout_ms`.
fn review_overdue(waited_ms: u64, timeout_ms: u64) -> bool {
    waited_ms >= timeout_ms
}

/// What a review approval's subject says about the task it rules on.
fn review_subject(task: &str) -> String {
    format!("review of task {task}")
}

/// The owner approval that asks for a ruling on `task`, or None while one is already pending for it.
/// The caller publishes what it returns.
fn escalate_review(
    approvals: &ApprovalStore,
    task: &str,
    worker: u64,
    worker_name: &str,
    goal: &str,
    summary: &str,
    waited_ms: u64,
) -> Option<Approval> {
    let subject = review_subject(task);
    let pending = approvals
        .list()
        .iter()
        .any(|a| a.kind == REVIEW_KIND && a.subject == subject && a.state == ApprovalState::Pending);
    if pending {
        return None;
    }
    let detail = json!({
        "task": task,
        "cid": worker,
        "worker": worker_name,
        "goal": goal,
        "summary": summary,
        "waited_ms": waited_ms,
    });
    Some(approvals.submit(worker, REVIEW_KIND, &subject, detail))
}

/// The reason a reviewer's ruling gives for withdrawing the owner's pending review card.
const RULED_FIRST: &str = "a reviewer ruled first";

/// The reason a card gets when its task ended by another path before the owner ruled.
const ENDED_FIRST: &str = "the task ended before the owner ruled";

/// Withdraws the pending review approvals for `task` with `reason`, and returns them.
fn withdraw_pending_review(approvals: &ApprovalStore, task: &str, reason: &str) -> Vec<Approval> {
    let subject = review_subject(task);
    approvals
        .list()
        .into_iter()
        .filter(|a| a.kind == REVIEW_KIND && a.subject == subject && a.state == ApprovalState::Pending)
        .filter_map(|a| approvals.withdraw(&a.id, reason))
        .collect()
}

/// Withdraws every pending review approval whose task `awaiting` says is no longer awaiting review, so
/// the owner is never left with a card that would be refused. Returns what it withdrew.
fn withdraw_orphaned_reviews(approvals: &ApprovalStore, awaiting: &dyn Fn(&str) -> bool) -> Vec<Approval> {
    let orphaned: Vec<(String, String)> = approvals
        .list()
        .into_iter()
        .filter(|a| a.kind == REVIEW_KIND && a.state == ApprovalState::Pending)
        .filter_map(|a| {
            let task = a.detail.get("task")?.as_str()?.to_string();
            Some((a.id, task))
        })
        .filter(|(_, task)| !awaiting(task))
        .collect();
    orphaned
        .into_iter()
        .filter_map(|(id, _)| approvals.withdraw(&id, ENDED_FIRST))
        .collect()
}

/// Publishes the approvals that were withdrawn, so the owner's view drops them.
fn publish_withdrawn(app: &App, withdrawn: &[Approval]) {
    for item in withdrawn {
        app.bus.publish("approval", item);
    }
}

/// Withdraws the pending review cards whose task no longer awaits review, whichever path ended it.
fn withdraw_stale_reviews(app: &App) {
    let awaiting = |task: &str| {
        app.plaza
            .tasks
            .find(task)
            .is_some_and(|record| record.state() == TaskState::AwaitingReview.as_str())
    };
    publish_withdrawn(app, &withdraw_orphaned_reviews(&app.approvals, &awaiting));
}

/// The owner's decision on a review approval (see `approvals::decide`): approve passes the task, deny
/// fails it. A task that is no longer awaiting review is refused.
pub fn review_decided(app: &App, item: &Approval, approve: bool) -> Result<(), String> {
    let task = item
        .detail
        .get("task")
        .and_then(Value::as_str)
        .ok_or("this review approval names no task")?;
    let notes = item.reason.clone().unwrap_or_else(|| "the owner ruled on the review".to_string());
    app.work.owner_verdict(app, task, approve, &notes).map(|_| ())
}

/// The scheduler thread (every 2 s).
pub fn spawn(app: Arc<App>) {
    // contract
    let spawned = thread::Builder::new().name("apid-scheduler".into()).spawn(move || {
        let mut motions_seen: HashSet<(String, u64)> = HashSet::new();
        loop {
            // While STOP ALL holds the plaza, nothing new starts; usage still drains.
            if !app.control.paused() {
                ask_names(&app);
                crate::society::outreach(&app);
                assign_tasks(&app);
                app.work.assign_reviews(&app);
                withdraw_stale_reviews(&app);
                offer_motions(&app, &mut motions_seen);
            }
            crate::llm_proxy::drain_usage(&app);
            crate::plaza::retain(&app);
            crate::usage::retain(&app);
            thread::sleep(Duration::from_secs(2));
        }
    });
    if let Err(err) = spawned {
        eprintln!("scheduler: cannot start: {err}");
    }
}

/// Asks each unnamed agent, once and only after its session settled, to name itself.
fn ask_names(app: &App) {
    let now = apz_plaza::now_ms();
    for agent in app.agents.list() {
        let settled = now.saturating_sub(agent.connected_at_ms) > 2_000;
        if agent.name.is_empty() && agent.status == crate::agents::Status::Idle && settled && app.names.first_ask(agent.cid) {
            let prompt = "Choose a short first name for yourself (one word, letters only, 2–16 characters, not a job title) and set it with plaza_set_name. Then reply with just your name.";
            app.work.enqueue(app, agent.cid, WorkKind::Chat, format!("op-{}", agent.cid), prompt.to_owned(), Origin::default());
        }
    }
}

/// Gives each open task with no work behind it to an idle worker of its role.
fn assign_tasks(app: &App) {
    for task in app.plaza.tasks.list() {
        if !task.is_open() {
            continue;
        }
        let busy = app.work.locked().holds_task(task.id());
        if busy {
            continue;
        }
        let role = task.role().map(str::to_string);
        let worker = app.agents.list().into_iter().find(|a| {
            a.status == Status::Idle
                && tier_of(&a.role) == Tier::Worker
                && role.as_deref().is_none_or(|r| a.role == r)
        });
        let Some(worker) = worker else {
            continue;
        };
        if task.claim(&connected_agent(&worker)).is_err() {
            continue;
        }
        let prompt = match task.acceptance() {
            Some(acceptance) => format!("{}\n\nAcceptance: {acceptance}", task.goal()),
            None => task.goal().to_string(),
        };
        let origin = Origin { task: Some(task.id().to_string()), ..Origin::default() };
        app.work.enqueue(app, worker.cid, WorkKind::Task, format!("task-{}", task.id()), prompt, origin);
    }
}

/// Asks each seated, connected agent that has not voted on an open motion
/// to vote, once per (motion, agent).
fn offer_motions(app: &App, seen: &mut HashSet<(String, u64)>) {
    let view = app.plaza.council.motions(MOTION_SCAN_LIMIT);
    for motion in view.motions.iter().filter(|m| m.state == "open") {
        for agent in app.agents.list() {
            if agent.status == Status::Offline || !is_seated(&agent.role) {
                continue;
            }
            if motion.votes.iter().any(|v| v.agent_cid == agent.cid) {
                continue;
            }
            if !seen.insert((motion.id.clone(), agent.cid)) {
                continue;
            }
            let prompt = format!("Council motion {} ({}): {}", motion.id, motion.kind, motion.text);
            let origin = Origin { motion: Some(motion.id.clone()), ..Origin::default() };
            app.work.enqueue(app, agent.cid, WorkKind::Motion, format!("motion-{}", motion.id), prompt, origin);
        }
    }
}


/// The saved form of the queues: the id counter and every lane (queue and item in flight).
#[derive(Serialize, Deserialize)]
struct Saved {
    next_id: u64,
    lanes: BTreeMap<u64, Lane>,
}

/// Prefixed to work that was in flight when the plaza stopped, so the agent knows to pick it up.
const RESUMED_PREFIX: &str = "[resumed after a plaza restart — continue from where you were; your workspace is as you left it]\n\n";

impl Queues {
    /// The whole state as JSON, for Postgres (`crate::db`); "" when there is nothing to save.
    pub fn snapshot(&self) -> String {
        let lanes = self.locked().clone();
        let next_id = self.next_id.load(Ordering::Relaxed);
        if next_id == 1 && lanes.lanes.values().all(|l| l.current.is_none() && l.queue.is_empty()) {
            return String::new();
        }
        serde_json::to_string(&Saved { next_id, lanes: lanes.lanes }).expect("work queues serialize")
    }

    /// Replaces the state with a [`Queues::snapshot`]; a refusal changes nothing.
    /// Work that was in flight goes back to the front of its agent's queue, marked
    /// as resumed. Nothing is sent: the queues dispatch as agents are idle.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let (next_id, mut lanes) = if json.is_empty() {
            (1, Lanes::default())
        } else {
            let saved: Saved = serde_json::from_str(json).map_err(|e| format!("work queues: {e}"))?;
            (saved.next_id, Lanes { lanes: saved.lanes })
        };
        for lane in lanes.lanes.values_mut() {
            if let Some(mut item) = lane.current.take() {
                item.started_ms = None;
                if !item.prompt.starts_with(RESUMED_PREFIX) {
                    item.prompt = format!("{RESUMED_PREFIX}{}", item.prompt);
                }
                lane.queue.push_front(item);
            }
        }
        let highest = lanes.lanes.values().flat_map(|l| l.queue.iter()).map(|i| i.id).max().unwrap_or(0);
        let next_id = next_id.max(highest.saturating_add(1));
        *self.locked() = lanes;
        self.next_id.store(next_id, Ordering::Relaxed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u64, cid: u64, task: Option<&str>) -> WorkItem {
        WorkItem {
            id,
            cid,
            kind: WorkKind::Task,
            thread: format!("task-{id}"),
            prompt: String::new(),
            origin: Origin { task: task.map(str::to_string), ..Origin::default() },
            created_ms: 0,
            started_ms: None,
        }
    }

    fn agent(cid: u64, role: &str, status: Status) -> Agent {
        Agent {
            cid,
            name: format!("agent{cid}"),
            role: role.to_string(),
            model_override: None,
            status,
            connected_at_ms: 0,
            last_seen_ms: 0,
            run: None,
            vm: None,
            usage: Default::default(),
            species: String::new(),
            kind: "llm".to_string(),
        }
    }

    #[test]
    fn a_done_task_goes_to_the_next_idle_reviewer_that_is_not_its_worker() {
        let agents = vec![
            agent(1, "builder", Status::Idle),
            agent(2, REVIEWER_ROLE, Status::Working),
            agent(3, REVIEWER_ROLE, Status::Idle),
            agent(4, REVIEWER_ROLE, Status::Idle),
        ];
        // The busy reviewer is skipped; the first free one takes it.
        assert_eq!(next_reviewer(&agents, 1), Some(3));
        // A reviewer does not review its own work.
        let own = vec![agent(5, REVIEWER_ROLE, Status::Idle)];
        assert_eq!(next_reviewer(&own, 5), None);
        // With no free reviewer the task keeps waiting.
        let busy = vec![agent(2, REVIEWER_ROLE, Status::Working), agent(6, "builder", Status::Idle)];
        assert_eq!(next_reviewer(&busy, 1), None);
    }

    #[test]
    fn a_task_no_reviewer_takes_in_time_asks_the_owner_once() {
        let dir = std::env::temp_dir().join(format!("apid-review-escalation-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = ApprovalStore::load(&dir);

        // Nothing is asked before the timeout; at it, the owner is.
        assert!(!review_overdue(29 * 60_000, 30 * 60_000));
        assert!(review_overdue(30 * 60_000, 30 * 60_000));

        let asked = escalate_review(&store, "t7", 4, "agent4", "ship it", "done", 30 * 60_000)
            .expect("the first escalation asks the owner");
        assert_eq!(asked.kind, "review");
        assert_eq!(asked.state, ApprovalState::Pending);
        assert_eq!(asked.cid, 4);
        assert_eq!(asked.subject, "review of task t7");
        assert_eq!(asked.detail["task"], "t7");
        assert_eq!(asked.detail["summary"], "done");
        assert_eq!(asked.detail["waited_ms"], 30 * 60_000);

        // While it is pending, later ticks do not ask again for the same task.
        assert!(escalate_review(&store, "t7", 4, "agent4", "ship it", "done", 40 * 60_000).is_none());
        // Another task gets its own approval.
        assert!(escalate_review(&store, "t8", 4, "agent4", "other", "done", 30 * 60_000).is_some());
        assert_eq!(store.list().iter().filter(|a| a.kind == REVIEW_KIND).count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reviewers_ruling_withdraws_the_owners_pending_card_and_only_that_one() {
        let dir = std::env::temp_dir().join(format!("apid-review-withdraw-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = ApprovalStore::load(&dir);
        let asked = escalate_review(&store, "t7", 4, "agent4", "ship it", "done", 30 * 60_000).expect("asked");
        let other = escalate_review(&store, "t8", 4, "agent4", "other", "done", 30 * 60_000).expect("asked");

        // A reviewer rules on t7 after the escalation: its card is withdrawn with the reason.
        let withdrawn = withdraw_pending_review(&store, "t7", RULED_FIRST);
        assert_eq!(withdrawn.len(), 1);
        let card = store.list().into_iter().find(|a| a.id == asked.id).expect("the card is kept");
        assert_eq!(card.state, ApprovalState::Withdrawn);
        assert_eq!(card.reason.as_deref(), Some("a reviewer ruled first"));

        // The owner can no longer decide it, and a second ruling withdraws nothing more.
        assert!(matches!(
            store.decide(&asked.id, true, None),
            Err(crate::approvals::DecideError::NotPending)
        ));
        assert!(withdraw_pending_review(&store, "t7", RULED_FIRST).is_empty());

        // t8's card is untouched and still pending.
        let still = store.list().into_iter().find(|a| a.id == other.id).expect("kept");
        assert_eq!(still.state, ApprovalState::Pending);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_card_whose_task_ended_by_another_path_is_withdrawn_too() {
        let dir = std::env::temp_dir().join(format!("apid-review-orphan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = ApprovalStore::load(&dir);
        escalate_review(&store, "t7", 4, "agent4", "a", "done", 30 * 60_000).expect("asked");
        let live = escalate_review(&store, "t8", 4, "agent4", "b", "done", 30 * 60_000).expect("asked");

        // Only t8 still awaits review (say t7 failed when its reviewer went offline).
        let awaiting = |task: &str| task == "t8";
        let withdrawn = withdraw_orphaned_reviews(&store, &awaiting);
        assert_eq!(withdrawn.len(), 1);
        assert_eq!(withdrawn[0].detail["task"], "t7");
        assert_eq!(withdrawn[0].reason.as_deref(), Some("the task ended before the owner ruled"));
        let kept = store.list().into_iter().find(|a| a.id == live.id).expect("kept");
        assert_eq!(kept.state, ApprovalState::Pending);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_in_flight_and_fifo() {
        let mut lanes = Lanes::default();
        for id in 1..=3 {
            lanes.push(item(id, 7, None));
        }
        assert_eq!(lanes.start_next(7, 10).map(|i| i.id), Some(1));
        assert!(lanes.start_next(7, 11).is_none(), "one item in flight per agent");
        assert_eq!(lanes.queued(7).len(), 2);
        assert_eq!(lanes.finish(7, 1).map(|i| i.id), Some(1));
        assert_eq!(lanes.start_next(7, 12).map(|i| i.id), Some(2));
    }

    #[test]
    fn finish_only_matches_the_item_in_flight() {
        let mut lanes = Lanes::default();
        lanes.push(item(1, 7, None));
        assert!(lanes.current(7).is_none());
        lanes.start_next(7, 0);
        assert!(lanes.finish(7, 99).is_none(), "a stray work id closes nothing");
        assert_eq!(lanes.current(7).map(|i| i.id), Some(1));
    }

    #[test]
    fn lanes_are_independent_per_agent() {
        let mut lanes = Lanes::default();
        lanes.push(item(1, 7, None));
        lanes.push(item(2, 8, None));
        lanes.start_next(7, 0);
        assert_eq!(lanes.start_next(8, 0).map(|i| i.id), Some(2));
    }

    #[test]
    fn failed_send_returns_item_to_front() {
        let mut lanes = Lanes::default();
        lanes.push(item(1, 7, None));
        lanes.push(item(2, 7, None));
        let first = lanes.start_next(7, 0).expect("started");
        lanes.requeue_front(first);
        assert!(lanes.current(7).is_none());
        assert_eq!(lanes.start_next(7, 5).map(|i| i.id), Some(1));
    }

    #[test]
    fn drain_takes_flight_then_queue_and_task_is_held_until_then() {
        let mut lanes = Lanes::default();
        lanes.push(item(1, 7, Some("t1")));
        lanes.push(item(2, 7, None));
        lanes.start_next(7, 0);
        assert!(lanes.holds_task("t1"));
        let ids: Vec<u64> = lanes.drain(7).into_iter().map(|i| i.id).collect();
        assert_eq!(ids, vec![1, 2]);
        assert!(!lanes.holds_task("t1"));
    }

    #[test]
    fn snapshot_restores_queues_and_resumes_in_flight_first() {
        let queues = Queues::default();
        {
            let mut lanes = queues.locked();
            for (id, cid) in [(1, 7), (2, 7), (3, 8)] {
                let mut it = item(id, cid, None);
                it.prompt = format!("work {id}");
                lanes.push(it);
            }
            lanes.start_next(7, 0); // item 1 is in flight for agent 7
        }
        queues.next_id.store(4, Ordering::Relaxed);
        let json = queues.snapshot();
        assert!(!json.is_empty());

        let fresh = Queues::default();
        fresh.restore(&json).expect("restores");
        assert_eq!(fresh.queued(7).len(), 2, "in-flight item goes back to the queue");
        assert!(fresh.current(7).is_none(), "nothing is in flight after restore");
        assert_eq!(fresh.queued(8).len(), 1);
        let first = &fresh.queued(7)[0];
        assert_eq!(first.id, 1);
        assert_eq!(first.prompt, format!("{RESUMED_PREFIX}work 1"));
        assert_eq!(fresh.queued(7)[1].prompt, "work 2");
        assert!(fresh.next_id.load(Ordering::Relaxed) >= 4);

        // A refusal changes nothing.
        assert!(fresh.restore("not json").is_err());
        assert_eq!(fresh.queued(7).len(), 2);
        assert_eq!(fresh.queued(8).len(), 1);
    }
}
