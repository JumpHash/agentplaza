//! Work items: per-agent FIFO queues (one item in flight per agent), sending
//! them through the hub, closing them, and the scheduler that turns open
//! tasks and open motions into work.
//!
//! OWNER: slice "scheduler". Signatures used by other modules are marked
//! `// contract` — keep them.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use apz_plaza::council::is_seated;
use apz_plaza::tasks::FinishOutcome;
use apz_plaza::treasury::{PostError, MARGIN_CAP_PCT};
use apz_proto::wire::{HostMessage, WorkKind, WorkStatus};
use serde::Serialize;
use serde_json::json;

use crate::agents::{connected_agent, Status};
use crate::settings::{tier_of, Tier};
use crate::state::App;

/// How many council motions the scheduler looks at per tick.
const MOTION_SCAN_LIMIT: usize = 50;

/// The role that verifies done tasks before they pay.
pub const REVIEWER_ROLE: &str = "reviewer";

/// What a work item is about, beyond its prompt.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Origin {
    pub task: Option<String>,
    pub motion: Option<String>,
    /// For `message` work: the agent who wrote.
    pub from_cid: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
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
#[derive(Default)]
struct Lane {
    queue: VecDeque<WorkItem>,
    current: Option<WorkItem>,
}

/// The queue logic, free of the app so it can be tested on its own.
#[derive(Default)]
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
            // A worker's done is a claim: an idle reviewer rules on it first.
            let reviewed = matches!(status, WorkStatus::Done)
                && matches!(item.kind, WorkKind::Task)
                && self.start_review(app, task, cid, summary);
            if !reviewed {
                settle_task(app, task, status, summary);
                self.tell_poster(app, task, cid, state, summary);
            }
        }
        self.review_lost(app, item.id, "reviewer ended without a verdict");
        app.agents.update(cid, |a| a.status = Status::Idle);
        self.dispatch(app, cid);
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
            app.hub.post(&send_path(cid), &json!({ "message": message })),
            Ok((code, _)) if (200..300).contains(&code)
        )
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

    /// Hands a done task to an idle reviewer; false when none is connected.
    fn start_review(&self, app: &App, task: &str, worker: u64, summary: &str) -> bool {
        let Some(record) = app.plaza.tasks.find(task) else {
            return false;
        };
        let Some(reviewer) = app
            .agents
            .list()
            .into_iter()
            .find(|a| a.cid != worker && a.role == REVIEWER_ROLE && a.status == Status::Idle)
        else {
            return false;
        };
        let worker_name = app.agents.get(worker).map_or_else(|| format!("agent {worker}"), |a| a.name);
        let acceptance = record.acceptance().map_or_else(|| "none stated".to_string(), |a| a.to_string());
        let prompt = format!(
            "Verify task {task}: {goal}\nAcceptance: {acceptance}\nThe worker ({worker_name}) reported: {summary}\n\
             Check it (use plaza_get_file for shared files) and call plaza_verdict with pass true/false and short notes.",
            goal = record.goal(),
        );
        let work = self.enqueue(app, reviewer.cid, WorkKind::Task, format!("review-{task}"), prompt, Origin::default());
        self.reviews.lock().unwrap_or_else(|p| p.into_inner()).insert(task.to_string(), Review { worker, work });
        true
    }

    /// The reviewer's verdict on `task`, from review work `work`. Pass pays the
    /// worker its margin-capped share and completes the task; fail fails it (the
    /// escrow goes back to the plaza's budget).
    pub fn verdict(&self, app: &App, task: &str, work: u64, pass: bool, notes: &str) -> Result<String, String> {
        // contract
        let mut reviews = self.reviews.lock().unwrap_or_else(|p| p.into_inner());
        if !reviews.get(task).is_some_and(|r| r.work == work) {
            return Err(format!("no verdict is pending on task {task} from this review"));
        }
        let review = reviews.remove(task).ok_or_else(|| format!("no verdict is pending on task {task}"))?;
        drop(reviews);
        let paid = if pass { self.pay_worker(app, task, review.worker, work) } else { 0 };
        let status = if pass { WorkStatus::Done } else { WorkStatus::Failed };
        settle_task(app, task, status, notes);
        self.tell_poster(app, task, review.worker, if pass { "done" } else { "failed" }, notes);
        Ok(format!("verdict {}: paid {paid}", if pass { "pass" } else { "fail" }))
    }

    /// Ends a review whose reviewer ended, or went offline, without a verdict: the task fails.
    fn review_lost(&self, app: &App, work: u64, reason: &str) {
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

    /// Sends the next waiting item to `cid` if it is reachable and idle.
    fn dispatch(&self, app: &App, cid: u64) {
        if app.agents.get(cid).is_none_or(|a| a.status == Status::Offline) {
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
        let sent = app.hub.post(&send_path(cid), &json!({ "message": message }));
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

fn send_path(cid: u64) -> String {
    format!("/v1/agents/{cid}/send")
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

/// The scheduler thread (every 2 s).
pub fn spawn(app: Arc<App>) {
    // contract
    let spawned = thread::Builder::new().name("apid-scheduler".into()).spawn(move || {
        let mut motions_seen: HashSet<(String, u64)> = HashSet::new();
        loop {
            ask_names(&app);
            crate::society::outreach(&app);
            assign_tasks(&app);
            offer_motions(&app, &mut motions_seen);
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
}
