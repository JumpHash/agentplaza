//! Work items: per-agent FIFO queues (one item in flight per agent), sending
//! them through the hub, closing them, and the scheduler that turns open
//! tasks and done tasks awaiting review into work.
//!
//! OWNER: slice "scheduler". Signatures used by other modules are marked
//! `// contract` — keep them.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use apz_metrics::{EntityKind, Registry, SeriesKey};
use apz_plaza::tasks::{FinishOutcome, TaskState};
use apz_plaza::treasury::{MARGIN_CAP_PCT, PostError};
use apz_proto::wire::{HostMessage, WorkKind, WorkStatus};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agents::{Agent, Status};
use crate::approvals::{Approval, State as ApprovalState, Store as ApprovalStore};
use crate::state::App;

/// The role that verifies done tasks before they pay.
pub const REVIEWER_ROLE: &str = "reviewer";

/// What a work item is about, beyond its prompt.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Origin {
    pub task: Option<String>,
    /// For `message` work: the agent who wrote.
    pub from_cid: Option<u64>,
    /// True for the owner's own chat (the owner chat route sets it); automated chats leave it false.
    #[serde(default)]
    pub owner: bool,
    /// True when the owner's message came over the hub's MCP server; `message_orchestrator` sets it.
    #[serde(default)]
    pub mcp: bool,
    /// True when the owner's message came from an editor over ACP (`/v1/acp`).
    #[serde(default)]
    pub acp: bool,
    /// The owner's conversation with the orchestrator this message is in (`conversations`): its answer goes there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
    /// The task's lane in Weft, for a worker's task item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<apz_proto::wire::WorkLane>,
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
        self.lanes
            .entry(item.cid)
            .or_default()
            .queue
            .push_back(item);
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
        self.lanes
            .get(&cid)
            .map(|l| l.queue.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// How many items wait behind the one in flight for `cid`.
    fn waiting(&self, cid: u64) -> usize {
        self.lanes.get(&cid).map_or(0, |lane| lane.queue.len())
    }

    /// Takes item `work` out of `cid`'s queue, if it waits there. The item in flight is not queued, so it is not taken.
    fn remove_queued(&mut self, cid: u64, work: u64) -> Option<WorkItem> {
        let lane = self.lanes.get_mut(&cid)?;
        let at = lane.queue.iter().position(|item| item.id == work)?;
        lane.queue.remove(at)
    }

    /// Removes everything for `cid`: the item in flight first, then the queue.
    fn drain(&mut self, cid: u64) -> Vec<WorkItem> {
        let Some(lane) = self.lanes.remove(&cid) else {
            return Vec::new();
        };
        lane.current.into_iter().chain(lane.queue).collect()
    }

    /// Whether an item for plaza task `task` is in flight or waiting.
    /// The agent whose lane holds work item `work`, in flight or waiting.
    fn holder_of(&self, work: u64) -> Option<u64> {
        self.lanes.iter().find_map(|(cid, lane)| {
            lane.current
                .iter()
                .chain(lane.queue.iter())
                .any(|item| item.id == work)
                .then_some(*cid)
        })
    }

    /// Drops the waiting items of plaza task `task` from `cid`'s lane. Returns whether its item is in flight: the
    /// caller cancels that one (the in-flight item is not dropped here, it ends when the agent says so).
    fn drop_task(&mut self, cid: u64, task: &str) -> bool {
        let Some(lane) = self.lanes.get_mut(&cid) else {
            return false;
        };
        lane.queue
            .retain(|item| item.origin.task.as_deref() != Some(task));
        lane.current
            .as_ref()
            .is_some_and(|item| item.origin.task.as_deref() == Some(task))
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
    /// Work items interrupted to move their agent (`Queues::interrupt`): their end puts them back in the
    /// queue instead of ending them.
    interrupted: Mutex<BTreeSet<u64>>,
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
            interrupted: Mutex::default(),
        }
    }
}

impl Queues {
    /// The lock, recovered if a previous holder panicked.
    fn locked(&self) -> MutexGuard<'_, Lanes> {
        self.lanes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The interrupted items, recovered if a previous holder panicked.
    fn interrupted(&self) -> MutexGuard<'_, BTreeSet<u64>> {
        self.interrupted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Queues work for `cid` and sends it if the agent is idle; the work id.
    pub fn enqueue(
        &self,
        app: &App,
        cid: u64,
        kind: WorkKind,
        thread: String,
        prompt: String,
        origin: Origin,
    ) -> u64 {
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
        set_queued_gauge(&app.metrics.registry, &self.locked(), cid);
        id
    }

    /// The item in flight for `cid`.
    pub fn current(&self, cid: u64) -> Option<WorkItem> {
        // contract
        self.locked().current(cid)
    }

    /// The reviewer on `task` while it awaits a verdict: the agent whose lane holds its review.
    pub fn reviewer_of(&self, task: &str) -> Option<u64> {
        let work = self
            .reviews
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(task)
            .map(|review| review.work)?;
        self.locked().holder_of(work)
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
        let interrupted = self.interrupted().remove(&work);
        // A turn interrupted for a move that ended anyway (done) ends as usual.
        if interrupted && !matches!(status, WorkStatus::Done) {
            self.resume_elsewhere(app, cid, item);
            return;
        }
        let state = status_str(status);
        // Stopped work was abandoned, so it counts as failed.
        let outcome = match status {
            WorkStatus::Done => "work.done",
            WorkStatus::Failed | WorkStatus::Stopped => "work.failed",
        };
        let registry = &app.metrics.registry;
        registry.counter(agent_key(outcome, cid)).inc();
        let totals = app.usage.work_totals(cid, work);
        registry
            .hist(agent_key("work.turns", cid))
            .record(totals.calls as f64);
        registry
            .hist(agent_key("work.cost_usd", cid))
            .record(totals.usd);
        app.transcripts.record(
            &app.bus,
            cid,
            work,
            crate::transcript::Entry::WorkEnd {
                status: outcome_of(status),
                summary: summary.to_string(),
            },
        );
        app.bus.publish(
            "work",
            &json!({ "cid": cid, "work": work, "state": "ended", "status": state }),
        );
        crate::conversations::work_ended(app, &item, state, summary);
        // A chat item (thread `chat:<id>`): its answer goes into the chat; a seat message it carried was read.
        crate::work_loop::on_chat_work_end(app, cid, &item.thread, summary);
        crate::work_loop::on_item_done(app, work);
        // A session's step: the work loop moves the task's seats on and ends the session, whose VM it retires.
        if crate::work_loop::on_work_end(app, cid, work, status, summary) {
            return;
        }
        if let Some(task) = item.origin.task.as_deref()
            && !crate::work_loop::owns(app, task)
        {
            // A worker's done is a claim: the task waits for a reviewer's verdict, and its
            // escrow stays held until then (see `assign_reviews`).
            let waiting = matches!(status, WorkStatus::Done)
                && matches!(item.kind, WorkKind::Task)
                && self.await_review(app, task, summary);
            // The worker's lane is sealed for review: its last draft, signed with the worker's key.
            if waiting {
                crate::weft_hub::seal(app, task, cid, summary);
            }
            if !waiting {
                settle_task(app, task, status, summary);
                // A task the owner's takeover requeued is open again, so its end is not news for the poster yet.
                if app
                    .plaza
                    .tasks
                    .find(task)
                    .is_none_or(|record| record.state() != "open")
                {
                    self.tell_poster(app, task, cid, state, summary);
                }
            }
        }
        self.review_lost(app, item.id, "reviewer ended without a verdict");
        // A review graph step whose agent ended without a verdict goes to another agent, once.
        crate::review_graph::work_ended(app, item.id);
        app.agents.update(cid, |a| a.status = Status::Idle);
        // An agent on a stopping node leaves it now that its turn is over.
        if crate::mover::holds(app, cid) {
            crate::mover::kick();
        }
        self.dispatch(app, cid);
        set_queued_gauge(&app.metrics.registry, &self.locked(), cid);
        // This agent is free now: it may be the reviewer a waiting task was waiting for.
        self.assign_reviews(app);
    }

    /// Interrupts `cid`'s turn so its work goes on on another machine (`mover`, a stop `now`): the item in
    /// flight is cancelled, and when the agent ends it, it goes back to the front of the queue
    /// ([`Queues::resume_elsewhere`]) instead of ending. True when the cancel reached the agent; false with
    /// nothing in flight, or when it was interrupted already.
    pub fn interrupt(&self, app: &App, cid: u64, reason: &str) -> bool {
        let Some(item) = self.current(cid) else {
            return false;
        };
        if !self.interrupted().insert(item.id) {
            return false;
        }
        let sent = self.cancel(app, cid, reason);
        if !sent {
            self.interrupted().remove(&item.id);
        }
        sent
    }

    /// An item [`Queues::interrupt`] stopped goes back to the front of its agent's queue, marked as moved,
    /// and starts again once the agent is on its new machine. It has not ended: no task settles and nobody
    /// is told; the transcript says it was interrupted.
    fn resume_elsewhere(&self, app: &App, cid: u64, mut item: WorkItem) {
        item.prompt = resumed(&item.prompt, MOVED_PREFIX);
        let work = item.id;
        self.locked().requeue_front(item);
        app.transcripts.record(
            &app.bus,
            cid,
            work,
            crate::transcript::Entry::WorkInterrupted {
                reason: "its machine is stopping; the work resumes on another machine".to_string(),
            },
        );
        app.bus.publish(
            "work",
            &json!({ "cid": cid, "work": work, "state": "interrupted" }),
        );
        app.agents.update(cid, |a| a.status = Status::Idle);
        // Held while the agent is on its stopping node; sent at once if the owner resumed it meanwhile.
        self.dispatch(app, cid);
        set_queued_gauge(&app.metrics.registry, &self.locked(), cid);
        crate::mover::kick();
    }

    /// Records that `cid` posted `task`, so it hears when the task ends.
    pub fn note_poster(&self, task: &str, cid: u64) {
        self.posters
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(task.to_string(), cid);
    }

    /// The agent that posted `task`, while its end has not been told yet.
    pub fn poster_of(&self, task: &str) -> Option<u64> {
        self.posters
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(task)
            .copied()
    }

    /// Tells a task's poster (the orchestrator) that it ended, continuing the
    /// poster's conversation with the operator so it keeps its plan.
    fn tell_poster(&self, app: &App, task: &str, worker: u64, state: &str, summary: &str) {
        let poster = self
            .posters
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(task);
        let Some(poster) = poster.filter(|&p| p != worker) else {
            return;
        };
        // A poster that may not work now (it left the project) is not told: its queue would only hold it.
        if crate::membership::held_cid(app, poster).is_some() {
            return;
        }
        let worker_name = app
            .agents
            .get(worker)
            .map_or_else(|| format!("agent {worker}"), |a| a.label());
        let prompt = format!(
            "[plaza] Task {task}, worked by {worker_name}, ended: {state}.\nSummary: {summary}\n\n\
             Review it (plaza_task_result), continue your plan, and tell the operator when the goal is complete."
        );
        // A project started from a conversation hears of its tasks there.
        if let Some(conversation) = project_conversation(app, task, poster) {
            crate::conversations::notify(
                app,
                &conversation,
                crate::conversations::shown_line(&prompt),
                &prompt,
            );
            return;
        }
        self.enqueue(
            app,
            poster,
            WorkKind::Chat,
            format!("op-{poster}"),
            prompt,
            Origin::default(),
        );
    }

    /// The operator cancels the current item (sends `cancel`).
    pub fn cancel(&self, app: &App, cid: u64, reason: &str) -> bool {
        // contract
        let Some(item) = self.locked().current(cid) else {
            return false;
        };
        let message = HostMessage::Cancel {
            work: item.id,
            reason: reason.to_string(),
        };
        matches!(
            crate::fleet::send(app, cid, &json!({ "message": message })),
            Ok((code, _)) if (200..300).contains(&code)
        )
    }

    /// Cancels item `work` of `cid` while it still waits in the queue: it leaves the queue and ends as `stopped`,
    /// recorded the way [`Queues::agent_gone`] ends a dropped item, so an editor's prompt ends with `cancelled`.
    /// The item in flight is [`Queues::cancel`]'s; false when `work` is not queued for `cid`.
    pub fn cancel_queued(&self, app: &App, cid: u64, work: u64, reason: &str) -> bool {
        let Some(item) = self.locked().remove_queued(cid, work) else {
            return false;
        };
        app.transcripts.record(
            &app.bus,
            cid,
            work,
            crate::transcript::Entry::WorkEnd {
                status: crate::transcript::Outcome::Stopped,
                summary: reason.to_string(),
            },
        );
        app.bus.publish(
            "work",
            &json!({ "cid": cid, "work": work, "state": "ended", "status": "stopped" }),
        );
        crate::conversations::work_ended(app, &item, "stopped", reason);
        if let Some(task) = item.origin.task.as_deref() {
            settle_task(app, task, WorkStatus::Stopped, reason);
        }
        self.review_lost(app, work, reason);
        true
    }

    /// Takes plaza task `task` off agent `cid`'s lane: its waiting items are dropped, and its item in flight is
    /// cancelled (`cancel`), so the agent stops working on it.
    fn drop_task(&self, app: &App, cid: u64, task: &str, reason: &str) {
        let in_flight = self.locked().drop_task(cid, task);
        if in_flight {
            self.cancel(app, cid, reason);
        }
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
            self.interrupted().remove(&item.id);
            app.transcripts.record(
                &app.bus,
                cid,
                item.id,
                crate::transcript::Entry::WorkEnd {
                    status: crate::transcript::Outcome::Failed,
                    summary: "agent went offline".to_string(),
                },
            );
            app.bus.publish(
                "work",
                &json!({ "cid": cid, "work": item.id, "state": "ended", "status": "failed" }),
            );
            crate::conversations::work_ended(app, &item, "failed", "agent went offline");
            if let Some(task) = item.origin.task.as_deref()
                && !crate::work_loop::owns(app, task)
            {
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
        self.spend
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(task)
            .copied()
            .unwrap_or(0)
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
        let now = apz_plaza::now_ms();
        for task in app.plaza.tasks.list() {
            let view = task.view();
            // A project's tasks wait as long as the project says, else the plaza.
            let timeout_ms = crate::project_settings::review_timeout_ms(app, view.project);
            if view.state != TaskState::AwaitingReview.as_str() {
                continue;
            }
            let Some(worker) = view.agent_cid else {
                continue;
            };
            let id = view.id.to_string();
            // A task the work loop runs is reviewed by its reviewer seat.
            if crate::work_loop::owns(app, &id) {
                continue;
            }
            // A task whose lane is sealed in Weft is reviewed by the review graph (review_graph::tick).
            if crate::review_graph::handles(app, &id) {
                continue;
            }
            let summary = view.note.clone().unwrap_or_default();
            let reviewed = self
                .reviews
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .contains_key(&id);
            if reviewed || self.start_review(app, &id, worker, &summary) {
                continue;
            }
            let waited = now.saturating_sub(view.updated_at);
            if review_overdue(waited, timeout_ms) {
                let worker_name = app
                    .agents
                    .get(worker)
                    .map_or_else(|| format!("agent {worker}"), |a| a.label());
                let escalation = escalate_review(
                    &app.approvals,
                    &id,
                    worker,
                    &worker_name,
                    view.goal,
                    &summary,
                    waited,
                );
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
        // The review is the task's project's: only a reviewer working on it takes it.
        let Some(project) = record.project().map(str::to_string) else {
            return false;
        };
        if crate::membership::project_state(app, &project)
            != crate::membership::ProjectState::Active
        {
            return false;
        }
        let members = app.members.members(&project);
        let reviewers = app.agents.select(|a| {
            (a.role == REVIEWER_ROLE && a.status == Status::Idle && members.contains(&a.cid))
                .then(|| a.clone())
        });
        let Some(reviewer) = next_reviewer(&reviewers, worker) else {
            return false;
        };
        let worker_name = app
            .agents
            .get(worker)
            .map_or_else(|| format!("agent {worker}"), |a| a.label());
        let acceptance = record
            .acceptance()
            .map_or_else(|| "none stated".to_string(), |a| a.to_string());
        let prompt = format!(
            "Verify task {task}: {goal}\nAcceptance: {acceptance}\nThe worker ({worker_name}) reported: {summary}\n\
             Check it (use plaza_get_file for shared files) and call plaza_verdict with pass true/false and short notes.",
            goal = record.goal(),
        );
        let work = self.enqueue(
            app,
            reviewer,
            WorkKind::Task,
            format!("review-{task}"),
            prompt,
            Origin::default(),
        );
        self.reviews
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(task.to_string(), Review { worker, work });
        true
    }

    /// The reviewer's verdict on `task`, from review work `work`. Pass pays the
    /// worker its margin-capped share and completes the task; fail fails it (the
    /// escrow goes back to the plaza's budget).
    pub fn verdict(
        &self,
        app: &App,
        task: &str,
        work: u64,
        pass: bool,
        notes: &str,
    ) -> Result<String, String> {
        // contract
        let rulings = self.rulings.lock().unwrap_or_else(|p| p.into_inner());
        let mut reviews = self.reviews.lock().unwrap_or_else(|p| p.into_inner());
        if reviews.get(task).is_none_or(|r| r.work != work) {
            return Err(format!(
                "no verdict is pending on task {task} from this review"
            ));
        }
        let review = reviews
            .remove(task)
            .ok_or_else(|| format!("no verdict is pending on task {task}"))?;
        drop(reviews);
        let summary = self.settle_ruling(app, task, review.worker, work, pass, notes);
        // The owner's card for this task, if one was raised, is no longer needed.
        publish_withdrawn(
            app,
            &withdraw_pending_review(&app.approvals, task, RULED_FIRST),
        );
        drop(rulings);
        self.tell_poster(
            app,
            task,
            review.worker,
            if pass { "done" } else { "failed" },
            notes,
        );
        Ok(summary)
    }

    /// The owner's ruling on `task`, which is awaiting review: a pass pays the worker and completes the
    /// task, a fail fails it. A reviewer still working on it is no longer awaited.
    pub fn owner_verdict(
        &self,
        app: &App,
        task: &str,
        pass: bool,
        notes: &str,
    ) -> Result<String, String> {
        let rulings = self.rulings.lock().unwrap_or_else(|p| p.into_inner());
        let record = app
            .plaza
            .tasks
            .find(task)
            .ok_or_else(|| format!("no task {task}"))?;
        let view = record.view();
        if view.state != TaskState::AwaitingReview.as_str() {
            return Err(format!(
                "task {task} is {}; it is not awaiting review",
                view.state
            ));
        }
        let worker = view
            .agent_cid
            .ok_or_else(|| format!("task {task} names no worker"))?;
        let reviewer = self
            .reviews
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(task);
        let work = reviewer.map_or(0, |r| r.work);
        let summary = self.settle_ruling(app, task, worker, work, pass, notes);
        drop(rulings);
        self.tell_poster(
            app,
            task,
            worker,
            if pass { "done" } else { "failed" },
            notes,
        );
        Ok(summary)
    }

    /// Pays the worker for a pass and ends the task, which releases the rest of its escrow. The caller
    /// holds `rulings`, so the task is paid at most once.
    fn settle_ruling(
        &self,
        app: &App,
        task: &str,
        worker: u64,
        work: u64,
        pass: bool,
        notes: &str,
    ) -> String {
        let paid = if pass {
            self.pay_worker(app, task, worker, work)
        } else {
            0
        };
        let status = if pass {
            WorkStatus::Done
        } else {
            WorkStatus::Failed
        };
        settle_task(app, task, status, notes);
        format!(
            "verdict {}: paid {paid}",
            if pass { "pass" } else { "fail" }
        )
    }

    /// Ends a review whose reviewer ended, or went offline, without a verdict: the task fails.
    fn review_lost(&self, app: &App, work: u64, reason: &str) {
        let rulings = self.rulings.lock().unwrap_or_else(|p| p.into_inner());
        let mut reviews = self.reviews.lock().unwrap_or_else(|p| p.into_inner());
        let found = reviews
            .iter()
            .find(|(_, r)| r.work == work)
            .map(|(task, _)| task.clone());
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
                Err(PostError::MarginCap {
                    paid_to_agents,
                    real_cost,
                    ..
                }) => {
                    let allowance = real_cost * i128::from(MARGIN_CAP_PCT) / 100 - paid_to_agents;
                    match i64::try_from(allowance) {
                        Ok(allowance)
                            if allowance > 0
                                && treasury.pay_agent(task, worker, allowance).is_ok() =>
                        {
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
            crate::transcript::Entry::Payment {
                task: task.to_string(),
                amount: paid,
                spent,
            },
        );
        paid
    }

    /// Starts the next item for every idle, connected agent that has queued work and nothing
    /// in flight — after a restart (restored queues) or anything else that left work waiting.
    pub fn dispatch_idle(&self, app: &App) {
        for cid in app.agents.cids_with(Status::Idle) {
            if self.current(cid).is_none() && !self.queued(cid).is_empty() {
                self.dispatch(app, cid);
            }
        }
    }

    /// Sends the next waiting item to `cid` if it is reachable and idle. An asleep agent is woken
    /// first and its work stays queued until its hello (`dispatch_idle`).
    fn dispatch(&self, app: &App, cid: u64) {
        let agent = app.agents.get(cid);
        let status = agent.as_ref().map(|a| a.status);
        // An agent on the bench, or on a paused project, keeps its work queued and is not woken for it.
        let held = agent
            .as_ref()
            .is_some_and(|a| crate::membership::held(app, a).is_some());
        match dispatch_step(
            status,
            app.control.paused() || held,
            crate::mover::holds(app, cid),
        ) {
            DispatchStep::Hold => return,
            DispatchStep::Wake => {
                crate::sleep::wake_with(app, cid, crate::sleep::WakePriority::Background);
                return;
            }
            DispatchStep::Send => {}
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
            lane: item.origin.lane.clone(),
        };
        let sent = crate::fleet::send(app, cid, &json!({ "message": message }));
        if !matches!(sent, Ok((code, _)) if (200..300).contains(&code)) {
            self.locked().requeue_front(item);
            app.agents.update_quietly(cid, |a| a.status = Status::Idle);
            set_queued_gauge(&app.metrics.registry, &self.locked(), cid);
            return;
        }
        let now = apz_plaza::now_ms();
        app.metrics
            .registry
            .hist(agent_key("work.wait_ms", cid))
            .record(now.saturating_sub(item.created_ms) as f64);
        set_queued_gauge(&app.metrics.registry, &self.locked(), cid);
        crate::metrics::collect::agent_state(app, cid, "first_token");
        app.transcripts.record(
            &app.bus,
            cid,
            item.id,
            crate::transcript::Entry::WorkStart {
                kind: item.kind.into(),
                thread: item.thread.clone(),
                prompt: item.prompt.clone(),
                origin: serde_json::to_value(&item.origin).unwrap_or(Value::Null),
            },
        );
        app.bus.publish(
            "work",
            &json!({ "cid": cid, "work": item.id, "state": "started" }),
        );
        crate::sleep::touch(app, cid);
    }
}

/// The conversation task `task`'s project was started from, when agent `cid` is the orchestrator it talks to: where
/// that orchestrator hears of the project's tasks.
pub(crate) fn project_conversation(app: &App, task: &str, cid: u64) -> Option<String> {
    let pid = app.plaza.tasks.find(task)?.project()?.to_string();
    crate::conversations::of_project(app, &pid)
        .filter(|conversation| conversation.cid == cid)
        .map(|conversation| conversation.id)
}

/// What `dispatch` does with an agent's waiting work, decided before anything moves.
#[derive(Debug, PartialEq, Eq)]
enum DispatchStep {
    /// Keep it queued: STOP ALL holds the plaza, the agent may not work now (on the bench, or its project
    /// paused, see `membership`), it is away (moving, or on a stopped node it leaves first:
    /// `mover::holds`), or it is gone (or unknown).
    Hold,
    /// Keep it queued and wake the agent: it is asleep (`crate::sleep`).
    Wake,
    /// Send the next item now.
    Send,
}

/// `held`: STOP ALL holds the plaza, or the agent may not work now (`membership`); `away`: it is moving,
/// or on a stopped node (`mover::holds`).
fn dispatch_step(status: Option<Status>, held: bool, away: bool) -> DispatchStep {
    match status {
        _ if held || away => DispatchStep::Hold,
        None | Some(Status::Offline) => DispatchStep::Hold,
        Some(Status::Asleep | Status::Dozing) => DispatchStep::Wake,
        Some(Status::Idle | Status::Working) => DispatchStep::Send,
    }
}

/// The series of metric `metric` for agent `cid`.
fn agent_key(metric: &str, cid: u64) -> SeriesKey {
    SeriesKey::new(metric, EntityKind::Agent, &cid.to_string())
}

/// Sets the `work.queued` gauge of `cid` to the items waiting behind its item in flight.
fn set_queued_gauge(registry: &Registry, lanes: &Lanes, cid: u64) {
    registry
        .gauge(agent_key("work.queued", cid))
        .set(lanes.waiting(cid) as f64);
}

/// The wire spelling of a work status, for the transcript and events.
/// The outcome a work item's status records (`work_end`).
fn outcome_of(status: WorkStatus) -> crate::transcript::Outcome {
    match status {
        WorkStatus::Done => crate::transcript::Outcome::Done,
        WorkStatus::Failed => crate::transcript::Outcome::Failed,
        WorkStatus::Stopped => crate::transcript::Outcome::Stopped,
    }
}

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
    let finished = task.finish(outcome, Some(summary.to_string())).is_ok();
    // A failed task's lane will never land.
    if finished && matches!(outcome, FinishOutcome::Failed) {
        crate::weft_hub::abandon(app, task.id(), summary);
    }
    // A task of a project that is over refunded its project's budget: that goes back to the plaza too.
    if let Some(pid) = task.project()
        && crate::membership::project_state(app, pid) == crate::membership::ProjectState::Over
    {
        crate::project_settings::return_rest(app, pid);
    }
}

/// Puts a task back in the queue when the owner takes the workstation its worker was using (the lease is released
/// with reason `owner_takeover`, takeover.rs): the task is `open` again with `reason` as its note, the worker's work
/// on it is cancelled, and the work loop runs it again. A task that is not claimed (it
/// ended, or it waits for a review) is left alone, and the error says so.
pub fn requeue_task(app: &App, task: &str, reason: &str) -> Result<(), String> {
    let record = app
        .plaza
        .tasks
        .find(task)
        .ok_or_else(|| format!("no task {task}"))?;
    let view = record.view();
    if view.state != "claimed" {
        return Err(format!("task {task} is {}, not claimed", view.state));
    }
    let worker = view.agent_cid;
    record.requeue(reason).map_err(|e| e.to_string())?;
    if let Some(cid) = worker {
        app.work.drop_task(app, cid, task, reason);
    }
    Ok(())
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
    let pending = approvals.list().iter().any(|a| {
        a.kind == REVIEW_KIND && a.subject == subject && a.state == ApprovalState::Pending
    });
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
        .filter(|a| {
            a.kind == REVIEW_KIND && a.subject == subject && a.state == ApprovalState::Pending
        })
        .filter_map(|a| approvals.withdraw(&a.id, reason))
        .collect()
}

/// Withdraws every pending review approval whose task `awaiting` says is no longer awaiting review, so
/// the owner is never left with a card that would be refused. Returns what it withdrew.
fn withdraw_orphaned_reviews(
    approvals: &ApprovalStore,
    awaiting: &dyn Fn(&str) -> bool,
) -> Vec<Approval> {
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
    let notes = item
        .reason
        .clone()
        .unwrap_or_else(|| "the owner ruled on the review".to_string());
    app.work
        .owner_verdict(app, task, approve, &notes)
        .map(|_| ())
}

/// Set when work may be able to start now (an agent turned idle); see `kick`.
static KICK: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

/// Asks the scheduler to start queued work now, rather than at its next full pass: an agent turned idle, so
/// it may take its next item. A kick runs only `Queues::dispatch_idle`; the full pass still runs every 2 s.
/// Kicks that arrive before the scheduler looks coalesce into one.
pub fn kick() {
    let (kicked, wake) = &KICK;
    *kicked.lock().unwrap_or_else(PoisonError::into_inner) = true;
    wake.notify_one();
}

/// Waits at most `timeout` for a kick; true when one came (it is consumed).
fn await_kick(timeout: Duration) -> bool {
    let (kicked, wake) = &KICK;
    let guard = kicked.lock().unwrap_or_else(PoisonError::into_inner);
    let (mut guard, _) = wake
        .wait_timeout_while(guard, timeout, |kicked| !*kicked)
        .unwrap_or_else(PoisonError::into_inner);
    std::mem::take(&mut *guard)
}

/// The scheduler thread (every 2 s).
pub fn spawn(app: Arc<App>) {
    // contract
    let spawned = thread::Builder::new()
        .name("apid-scheduler".into())
        .spawn(move || {
            loop {
                // While STOP ALL holds the plaza, nothing new starts; usage still drains.
                if !app.control.paused() {
                    // Tasks run as sessions filling seats (work_loop.rs).
                    crate::work_loop::tick(&app);
                    app.work.assign_reviews(&app);
                    crate::review_graph::tick(&app);
                    crate::weft_hub::land_queues(&app);
                    withdraw_stale_reviews(&app);
                }
                crate::llm_proxy::drain_usage(&app);
                crate::plaza::retain(&app);
                crate::usage::retain(&app);
                // The 2 s until the next pass: each kick in it starts the queued work of the idle agents.
                let next_pass = Instant::now() + Duration::from_secs(2);
                loop {
                    let left = next_pass.saturating_duration_since(Instant::now());
                    if left.is_zero() || !await_kick(left) {
                        break;
                    }
                    app.work.dispatch_idle(&app);
                }
            }
        });
    if let Err(err) = spawned {
        eprintln!("scheduler: cannot start: {err}");
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
/// Prefixed to work interrupted to move its agent off a stopping machine (`Queues::interrupt`). It also starts
/// again where it was when the owner resumes the machine before the agent moves, so the mark says neither.
const MOVED_PREFIX: &str = "[resumed after your turn was interrupted to move you off a stopping machine — continue from where you were; your workspace is as you left it]\n\n";

/// `prompt` marked with `prefix`, once: a mark it already has (either one) gives way to the new one.
fn resumed(prompt: &str, prefix: &str) -> String {
    let bare = prompt
        .strip_prefix(RESUMED_PREFIX)
        .or_else(|| prompt.strip_prefix(MOVED_PREFIX))
        .unwrap_or(prompt);
    format!("{prefix}{bare}")
}

impl Queues {
    /// The whole state as JSON, for Postgres (`crate::db`); "" when there is nothing to save.
    pub fn snapshot(&self) -> String {
        let lanes = self.locked().clone();
        let next_id = self.next_id.load(Ordering::Relaxed);
        if next_id == 1
            && lanes
                .lanes
                .values()
                .all(|l| l.current.is_none() && l.queue.is_empty())
        {
            return String::new();
        }
        serde_json::to_string(&Saved {
            next_id,
            lanes: lanes.lanes,
        })
        .expect("work queues serialize")
    }

    /// Replaces the state with a [`Queues::snapshot`]; a refusal changes nothing.
    /// Work that was in flight goes back to the front of its agent's queue, marked
    /// as resumed. Nothing is sent: the queues dispatch as agents are idle.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let (next_id, mut lanes) = if json.is_empty() {
            (1, Lanes::default())
        } else {
            let saved: Saved =
                serde_json::from_str(json).map_err(|e| format!("work queues: {e}"))?;
            (saved.next_id, Lanes { lanes: saved.lanes })
        };
        for lane in lanes.lanes.values_mut() {
            if let Some(mut item) = lane.current.take() {
                item.started_ms = None;
                item.prompt = resumed(&item.prompt, RESUMED_PREFIX);
                lane.queue.push_front(item);
            }
        }
        let highest = lanes
            .lanes
            .values()
            .flat_map(|l| l.queue.iter())
            .map(|i| i.id)
            .max()
            .unwrap_or(0);
        let next_id = next_id.max(highest.saturating_add(1));
        *self.locked() = lanes;
        self.next_id.store(next_id, Ordering::Relaxed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_queued_gauge_follows_the_lane_as_items_are_queued_and_started() {
        let registry = Registry::new(64);
        let mut lanes = Lanes::default();
        lanes.push(item(1, 7, None));
        lanes.push(item(2, 7, None));
        set_queued_gauge(&registry, &lanes, 7);
        assert_eq!(registry.gauge(agent_key("work.queued", 7)).get(), 2.0);

        assert!(lanes.start_next(7, 5).is_some());
        set_queued_gauge(&registry, &lanes, 7);
        assert_eq!(registry.gauge(agent_key("work.queued", 7)).get(), 1.0);
    }

    #[test]
    fn origin_without_acp_still_parses() {
        // Work items queued before `acp` existed carry no such key: they are not from an editor.
        let old = r#"{"task":null,"motion":null,"from_cid":null,"owner":true,"mcp":false}"#;
        let origin: Origin = serde_json::from_str(old).unwrap();
        assert!(origin.owner);
        assert!(!origin.acp);
    }

    fn item(id: u64, cid: u64, task: Option<&str>) -> WorkItem {
        WorkItem {
            id,
            cid,
            kind: WorkKind::Task,
            thread: format!("task-{id}"),
            prompt: String::new(),
            origin: Origin {
                task: task.map(str::to_string),
                ..Origin::default()
            },
            created_ms: 0,
            started_ms: None,
        }
    }

    fn agent(cid: u64, role: &str, status: Status) -> Agent {
        Agent {
            cid,
            role: role.to_string(),
            model_override: None,
            status,
            connected_at_ms: 0,
            last_seen_ms: 0,
            run: None,
            vm: None,
            usage: Default::default(),
            kind: "llm".to_string(),
            caps: Vec::new(),
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
        let busy = vec![
            agent(2, REVIEWER_ROLE, Status::Working),
            agent(6, "builder", Status::Idle),
        ];
        assert_eq!(next_reviewer(&busy, 1), None);
    }

    #[test]
    fn a_task_no_reviewer_takes_in_time_asks_the_owner_once() {
        let dir =
            std::env::temp_dir().join(format!("apid-review-escalation-{}", std::process::id()));
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
        assert!(
            escalate_review(&store, "t7", 4, "agent4", "ship it", "done", 40 * 60_000).is_none()
        );
        // Another task gets its own approval.
        assert!(escalate_review(&store, "t8", 4, "agent4", "other", "done", 30 * 60_000).is_some());
        assert_eq!(
            store
                .list()
                .iter()
                .filter(|a| a.kind == REVIEW_KIND)
                .count(),
            2
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reviewers_ruling_withdraws_the_owners_pending_card_and_only_that_one() {
        let dir = std::env::temp_dir().join(format!("apid-review-withdraw-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = ApprovalStore::load(&dir);
        let asked = escalate_review(&store, "t7", 4, "agent4", "ship it", "done", 30 * 60_000)
            .expect("asked");
        let other = escalate_review(&store, "t8", 4, "agent4", "other", "done", 30 * 60_000)
            .expect("asked");

        // A reviewer rules on t7 after the escalation: its card is withdrawn with the reason.
        let withdrawn = withdraw_pending_review(&store, "t7", RULED_FIRST);
        assert_eq!(withdrawn.len(), 1);
        let card = store
            .list()
            .into_iter()
            .find(|a| a.id == asked.id)
            .expect("the card is kept");
        assert_eq!(card.state, ApprovalState::Withdrawn);
        assert_eq!(card.reason.as_deref(), Some("a reviewer ruled first"));

        // The owner can no longer decide it, and a second ruling withdraws nothing more.
        assert!(matches!(
            store.decide(&asked.id, true, None),
            Err(crate::approvals::DecideError::NotPending)
        ));
        assert!(withdraw_pending_review(&store, "t7", RULED_FIRST).is_empty());

        // t8's card is untouched and still pending.
        let still = store
            .list()
            .into_iter()
            .find(|a| a.id == other.id)
            .expect("kept");
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
        let live =
            escalate_review(&store, "t8", 4, "agent4", "b", "done", 30 * 60_000).expect("asked");

        // Only t8 still awaits review (say t7 failed when its reviewer went offline).
        let awaiting = |task: &str| task == "t8";
        let withdrawn = withdraw_orphaned_reviews(&store, &awaiting);
        assert_eq!(withdrawn.len(), 1);
        assert_eq!(withdrawn[0].detail["task"], "t7");
        assert_eq!(
            withdrawn[0].reason.as_deref(),
            Some("the task ended before the owner ruled")
        );
        let kept = store
            .list()
            .into_iter()
            .find(|a| a.id == live.id)
            .expect("kept");
        assert_eq!(kept.state, ApprovalState::Pending);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dispatch_on_an_asleep_agent_wakes_it_and_keeps_the_work_queued() {
        assert_eq!(
            dispatch_step(Some(Status::Asleep), false, false),
            DispatchStep::Wake
        );
        assert_eq!(
            dispatch_step(Some(Status::Asleep), true, false),
            DispatchStep::Hold,
            "STOP ALL holds even a wake"
        );
        assert_eq!(
            dispatch_step(Some(Status::Dozing), false, false),
            DispatchStep::Wake,
            "work for a dozing agent wakes it (an undoze), it is not sent to the paused VM"
        );
        assert_eq!(
            dispatch_step(Some(Status::Dozing), true, false),
            DispatchStep::Hold
        );
        assert_eq!(dispatch_step(None, false, false), DispatchStep::Hold);
        assert_eq!(
            dispatch_step(Some(Status::Offline), false, false),
            DispatchStep::Hold
        );
        assert_eq!(
            dispatch_step(Some(Status::Idle), false, false),
            DispatchStep::Send
        );
        assert_eq!(
            dispatch_step(Some(Status::Working), false, false),
            DispatchStep::Send
        );
        assert_eq!(
            dispatch_step(Some(Status::Idle), true, false),
            DispatchStep::Hold,
            "an agent on the bench (or STOP ALL) keeps its work queued, and an asleep one is not woken for it"
        );
        assert_eq!(
            dispatch_step(Some(Status::Idle), false, true),
            DispatchStep::Hold,
            "an agent that is moving, or leaving a stopped node, starts nothing"
        );
        assert_eq!(
            dispatch_step(Some(Status::Asleep), false, true),
            DispatchStep::Hold,
            "nor is it woken where it is: it wakes on its new node"
        );

        // A wake sends nothing: the item waits in the lane, not in flight, until the agent is idle.
        let mut lanes = Lanes::default();
        lanes.push(item(1, 7, None));
        assert_eq!(lanes.queued(7).len(), 1);
        assert!(lanes.current(7).is_none());
        assert_eq!(lanes.start_next(7, 0).map(|i| i.id), Some(1));
    }

    #[test]
    fn an_interrupted_item_goes_back_first_and_is_marked_once() {
        let mut lanes = Lanes::default();
        lanes.push(item(1, 7, Some("t1")));
        lanes.push(item(2, 7, None));
        let mut started = lanes.start_next(7, 5).expect("in flight");
        assert_eq!(lanes.finish(7, 1).map(|i| i.id), Some(1));
        started.prompt = resumed(&started.prompt, MOVED_PREFIX);
        lanes.requeue_front(started);
        let queued = lanes.queued(7);
        assert_eq!(queued.iter().map(|i| i.id).collect::<Vec<_>>(), [1, 2]);
        assert!(queued[0].started_ms.is_none());
        assert!(lanes.current(7).is_none());
        // Marked once, whichever mark it had: a restart's and a move's do not pile up.
        let once = resumed(&queued[0].prompt, MOVED_PREFIX);
        assert_eq!(once, queued[0].prompt);
        let after_restart = resumed(&once, RESUMED_PREFIX);
        assert!(after_restart.starts_with(RESUMED_PREFIX));
        assert!(!after_restart.contains(MOVED_PREFIX));
        assert_eq!(
            resumed(&after_restart, MOVED_PREFIX),
            format!("{MOVED_PREFIX}{}", item(1, 7, None).prompt)
        );
    }

    #[test]
    fn one_in_flight_and_fifo() {
        let mut lanes = Lanes::default();
        for id in 1..=3 {
            lanes.push(item(id, 7, None));
        }
        assert_eq!(lanes.start_next(7, 10).map(|i| i.id), Some(1));
        assert!(
            lanes.start_next(7, 11).is_none(),
            "one item in flight per agent"
        );
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
        assert!(
            lanes.finish(7, 99).is_none(),
            "a stray work id closes nothing"
        );
        assert_eq!(lanes.current(7).map(|i| i.id), Some(1));
    }

    #[test]
    fn remove_queued_takes_a_waiting_item_and_never_the_one_in_flight() {
        let mut lanes = Lanes::default();
        for id in 1..=3 {
            lanes.push(item(id, 7, None));
        }
        lanes.start_next(7, 0);
        assert_eq!(lanes.remove_queued(7, 2).map(|i| i.id), Some(2));
        assert!(lanes.remove_queued(7, 1).is_none(), "in flight, not queued");
        assert!(lanes.remove_queued(7, 9).is_none(), "no such item");
        assert_eq!(
            lanes.queued(7).iter().map(|i| i.id).collect::<Vec<_>>(),
            vec![3]
        );
        assert_eq!(lanes.current(7).map(|i| i.id), Some(1));
    }

    #[test]
    fn a_work_item_is_found_in_the_lane_that_holds_it() {
        let mut lanes = Lanes::default();
        lanes.push(item(1, 7, None));
        lanes.push(item(2, 7, None));
        lanes.push(item(3, 8, None));
        assert!(lanes.start_next(7, 0).is_some());
        assert_eq!(lanes.holder_of(1), Some(7), "in flight");
        assert_eq!(lanes.holder_of(2), Some(7), "waiting");
        assert_eq!(lanes.holder_of(3), Some(8));
        assert_eq!(lanes.holder_of(4), None);
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
    fn drain_takes_the_item_in_flight_then_the_queue() {
        let mut lanes = Lanes::default();
        lanes.push(item(1, 7, Some("t1")));
        lanes.push(item(2, 7, None));
        lanes.start_next(7, 0);
        let ids: Vec<u64> = lanes.drain(7).into_iter().map(|i| i.id).collect();
        assert_eq!(ids, vec![1, 2]);
        assert!(lanes.current(7).is_none());
    }

    #[test]
    fn a_requeued_task_drops_its_waiting_items_and_says_whether_its_item_is_in_flight() {
        let mut lanes = Lanes::default();
        lanes.push(item(1, 7, Some("t1")));
        lanes.push(item(2, 7, Some("t2")));
        lanes.push(item(3, 7, Some("t1")));
        lanes.start_next(7, 0); // item 1 (t1) is in flight
        assert!(
            lanes.drop_task(7, "t1"),
            "t1's item is in flight: the caller cancels it"
        );
        assert_eq!(
            lanes.queued(7).iter().map(|i| i.id).collect::<Vec<_>>(),
            vec![2],
            "t1's waiting item 3 is gone, t2's stays"
        );
        assert_eq!(
            lanes.current(7).map(|i| i.id),
            Some(1),
            "the in-flight item is left for its end"
        );
        assert!(
            !lanes.drop_task(7, "t2"),
            "t2 has nothing in flight; its waiting item goes"
        );
        assert!(lanes.queued(7).is_empty());
        assert!(
            !lanes.drop_task(9, "t1"),
            "an agent with no lane has nothing to drop"
        );
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
        assert_eq!(
            fresh.queued(7).len(),
            2,
            "in-flight item goes back to the queue"
        );
        assert!(
            fresh.current(7).is_none(),
            "nothing is in flight after restore"
        );
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

#[cfg(test)]
mod kick_tests {
    use super::*;

    #[test]
    fn a_kick_wakes_the_wait_once() {
        // Other tests kick too (a roster change to Idle), so only the kick made here is asserted on.
        kick();
        assert!(await_kick(Duration::ZERO));
    }
}
