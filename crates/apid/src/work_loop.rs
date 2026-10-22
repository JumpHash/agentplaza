//! The work loop (Engine 0.3, 0.4): tasks run as sessions filling seats.
//!
//! Every tick (the scheduler's, every 2 s):
//! 1. a task that is ready to run (open, every `after` link done, its project active, no seats yet) gets its recipe's
//!    seats (`seats.rs`: the built-in change recipe) and reserves its budget from its project (`budgets.rs`); a project
//!    that cannot reserve it asks the owner with a `budget` card and the task waits;
//! 2. every ready seat gets a fresh session: a model (a reviewer's differs from the builder's, W27), a node, a VM
//!    started for it ("a fork from today's templates until phase 3");
//! 3. a session whose VM did not say hello in time fails, and its seat tries again.
//!
//! When the VM says hello ([`on_hello`]) the session runs: its step's work item (the task's goal and acceptance, what a
//! review sent back, the messages waiting on its seat) goes out on the VM's lane. When the step's work item ends
//! ([`on_work_end`]) or the reviewer gives its verdict ([`on_verdict`]), the seats move on (build → review → done, or
//! back to the builder with the findings), the session writes its handover and ends, and its VM is retired. A code
//! task's builder seals its Weft lane when it finishes, so the reviewer reviews a fixed revision; a pass joins the
//! project's landing queue and the task is done when its change lands ([`landed`]); a clash on landing sends it back to
//! the builder ([`clashed`]). The task's budget is released when it ends.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, PoisonError};

use apz_plaza::tasks::FinishOutcome;
use apz_proto::wire::{WorkKind, WorkStatus};

use crate::budgets::{Amount, Notice, OwnerKind, Short, Unit};
use crate::cards::{self, NewCard};
use crate::chats::{Author, MessageKind, Post};
use crate::scheduler::Origin;
use crate::seats::{self, Next, Seat};
use crate::sessions::{Kind, NewSession, PausedBy, Session, State};
use crate::state::App;

/// How long a session's VM may take from its start to its hello before the session fails.
pub const HELLO_DEADLINE_MS: u64 = 5 * 60_000;
/// What a task reserves from its project when neither the task nor the project says (micro-dollars, or tokens).
pub const DEFAULT_RESERVE_USD: u64 = 2_000_000;
pub const DEFAULT_RESERVE_TOKENS: u64 = 2_000_000;
/// The engine a session runs on until recipes choose (phase 4): the plaza's own harness.
pub const ENGINE: &str = "harness";
/// The longest answer a call that names no cap may get, for its budget hold: the largest default a provider answers
/// with (Anthropic's 32 000 tokens; the others' are lower).
pub const UNCAPPED_ANSWER_TOKENS: u64 = 32_000;

fn now() -> u64 {
    apz_plaza::now_ms()
}

/// Whether the work loop runs `task` (it has seats): the scheduler's old paths (the reviewer pool, the review graph)
/// leave it alone.
pub fn owns(app: &App, task: &str) -> bool {
    !app.seats.of_task(task).is_empty()
}

/// One tick of the loop.
pub fn tick(app: &App) {
    let now = now();
    ensure_planners(app, now);
    assign_tasks(app, now);
    for seat in app.seats.ready() {
        start_seat(app, &seat, now);
    }
    reap(app, now);
    crate::disk::tick(app, now);
}

/// Whether project `id` takes work: it exists, is not paused or archived, and is in a stage that runs tasks, or in
/// maintenance (its planner sweeps it, and the tasks the owner picks from its proposals run).
fn project_active(app: &App, id: &str) -> bool {
    app.projects.get(id).is_some_and(|p| {
        !p.paused
            && !p.archived
            && (p.in_maintenance() || matches!(p.stage.as_str(), "implementation" | "review"))
    })
}

/// What a task reserves from its project: its own cap when it has one in the project's unit, else the project's
/// default task budget, else a default.
fn reserve_for(app: &App, task: &str, project: &str) -> u64 {
    let project_unit = app
        .budgets
        .get(OwnerKind::Project, project)
        .map_or(Unit::Usd, |b| b.unit);
    if let Some(t) = app.budgets.get(OwnerKind::Task, task)
        && t.unit == project_unit
        && let Some(cap) = t.cap
    {
        return cap;
    }
    if let Some(budget) = crate::project_settings::task_budget(app, Some(project)) {
        return budget;
    }
    match project_unit {
        Unit::Usd => DEFAULT_RESERVE_USD,
        Unit::Tokens => DEFAULT_RESERVE_TOKENS,
    }
}

/// Fills the seats of every ready task (PLAN 0.4: `assign_tasks` fills seats): a task that is ready to run gets its
/// recipe's seats and reserves its budget; its seats get sessions on the loop's next steps.
fn assign_tasks(app: &App, now: u64) {
    for task in app.plaza.tasks.list() {
        if !task.is_open() || !task.is_ready() {
            continue;
        }
        let Some(project) = task.project().map(str::to_string) else {
            continue;
        };
        if !project_active(app, &project) || !app.seats.of_task(task.id()).is_empty() {
            continue;
        }
        let reserve = reserve_for(app, task.id(), &project);
        if let Err(short) = app.budgets.start_task(task.id(), reserve, now) {
            ask_budget(app, &short, Some(task.id()), &project, now);
            continue;
        }
        app.seats.open(task.id(), &seats::CHANGE, now);
        notice(
            app,
            &task_chat(app, task.id(), &project, now),
            &format!(
                "Task {} starts: a builder, then a review on another model.",
                task.id()
            ),
            now,
        );
    }
}

/// The models a seat may use, best first: the project's model for the role, the project's default, the plaza's model for
/// the role and its default, then the worker list.
fn candidates(app: &App, project: &str, role: &str) -> Vec<String> {
    let settings = app.settings.get();
    let project_settings = app.project_settings.get(project);
    let mut out: Vec<String> = Vec::new();
    let mut push = |m: Option<String>| {
        if let Some(m) = m.filter(|m| !m.is_empty())
            && !out.contains(&m)
        {
            out.push(m);
        }
    };
    push(project_settings.role_models.get(role).cloned());
    push(project_settings.default_model.clone());
    push(settings.role_models.get(role).cloned());
    push(Some(app.settings.resolve(role, None)));
    for m in &settings.model_policy.worker {
        push(Some(m.clone()));
    }
    out
}

fn provider_of(model: &str) -> &str {
    model.split('/').next().unwrap_or(model)
}

/// The model for `seat`: the first candidate; for a `differ` seat the project's own model for the role when no other
/// seat of the task used it, else the first candidate no other seat used, preferring another provider. None when a
/// `differ` seat has no model left.
pub fn pick_model(app: &App, project: &str, seat: &Seat) -> Option<String> {
    let all = candidates(app, project, &seat.role);
    if seat.model_policy == seats::ModelPolicy::Any {
        return all.into_iter().next();
    }
    let avoid: HashSet<String> = app
        .seats
        .models_to_avoid(&seat.task, &seat.name)
        .into_iter()
        .collect();
    if let Some(chosen) = app
        .project_settings
        .get(project)
        .role_models
        .get(&seat.role)
        .filter(|m| !m.is_empty() && !avoid.contains(*m))
    {
        return Some(chosen.clone());
    }
    let avoid_providers: HashSet<&str> = avoid.iter().map(|m| provider_of(m)).collect();
    let free: Vec<&String> = all.iter().filter(|m| !avoid.contains(*m)).collect();
    free.iter()
        .find(|m| !avoid_providers.contains(provider_of(m)))
        .or_else(|| free.first())
        .map(|m| (*m).clone())
}

/// Starts a session for a ready seat: picks its model, places it and starts its VM.
fn start_seat(app: &App, seat: &Seat, now: u64) {
    let Some(task) = app.plaza.tasks.find(&seat.task) else {
        app.seats.close(&seat.task);
        return;
    };
    let Some(project) = task.project().map(str::to_string) else {
        return;
    };
    let Some(model) = pick_model(app, &project, seat) else {
        ask_seat(app, seat, &project, now);
        return;
    };
    let session = app.sessions.create(
        NewSession {
            project: project.clone(),
            task: Some(seat.task.clone()),
            seat: Some(seat.name.clone()),
            kind: Kind::Step,
            parent: None,
            role: seat.role.clone(),
            engine: ENGINE.into(),
            model: Some(model.clone()),
            attempt: seat.attempts + 1,
        },
        now,
    );
    app.seats
        .fill(&seat.task, &seat.name, &session.id, Some(&model), now);
    let _ = app.sessions.set_state(&session.id, State::Placing, now);
    match crate::agents::spawn_agents(app, &seat.role, 1, Some(&model), None, Some(&project)) {
        Ok(started) => {
            let cid = started["cids"].get(0).and_then(serde_json::Value::as_u64);
            let node = started["node"].as_str().map(str::to_string);
            let Some(cid) = cid else {
                fail_session(app, &session, "the node started no VM", now);
                return;
            };
            let _ = app.sessions.update(&session.id, |s| {
                s.cid = Some(cid);
                s.node = node;
            });
            let _ = app.sessions.set_state(&session.id, State::Forking, now);
            app.bus.publish("session", &app.sessions.get(&session.id));
        }
        Err(why) => fail_session(app, &session, &why, now),
    }
}

/// A session that could not start, or whose VM went away: it fails and its seat goes again (or fails for good).
fn fail_session(app: &App, session: &Session, why: &str, now: u64) {
    let _ = app.sessions.finish(&session.id, why, true, now);
    if let Some(cid) = session.cid {
        retire(app, cid);
    }
    if let (Some(task), Some(seat)) = (&session.task, &session.seat)
        && let Some(next) = app.seats.ended(task, seat, false, true, why, now)
    {
        apply(app, task, next, now);
    }
    app.bus.publish("session", &app.sessions.get(&session.id));
}

/// Sessions whose VM did not say hello in time fail.
fn reap(app: &App, now: u64) {
    let stuck: Vec<Session> = app
        .sessions
        .list(&crate::sessions::Filter {
            state: Some(State::Forking),
            ..Default::default()
        })
        .into_iter()
        .filter(|s| now.saturating_sub(s.created_ms) > HELLO_DEADLINE_MS)
        .collect();
    for s in stuck {
        fail_session(app, &s, "its VM did not start in time", now);
    }
}

/// The step's prompt for a session that just started.
fn step_prompt(app: &App, session: &Session) -> String {
    let Some(task_id) = &session.task else {
        return String::new();
    };
    let Some(task) = app.plaza.tasks.find(task_id) else {
        return String::new();
    };
    let seat = session.seat.as_deref().unwrap_or("builder");
    let mut prompt = format!("Task {task_id}: {}\n", task.goal());
    if let Some(acceptance) = task.acceptance() {
        prompt.push_str(&format!("\nAcceptance:\n{acceptance}\n"));
    }
    match seat {
        "reviewer" => {
            // The builder's newest session (its seat let go of it when its step ended).
            let builder = app
                .sessions
                .list(&crate::sessions::Filter {
                    task: Some(task_id.clone()),
                    ..Default::default()
                })
                .into_iter()
                .filter(|s| s.seat.as_deref() == Some("builder"))
                .max_by_key(|s| s.created_ms);
            if let Some(handover) = builder.and_then(|s| s.handover) {
                prompt.push_str(&format!("\nThe builder's handover:\n{handover}\n"));
            }
            prompt.push_str(
                "\nYou are this task's reviewer, on a model other than the builder's. Try to break the change: check \
                 it against the acceptance, run what proves it, and look for what the builder missed. Then call \
                 plaza_verdict with pass true or false; when false, say exactly what must change.\n",
            );
        }
        _ => {
            if let Some(back) = app.seats.get(task_id, seat).and_then(|s| s.returned_with) {
                prompt.push_str(&format!("\nThe review sent the change back:\n{back}\n"));
            }
            prompt.push_str(
                "\nYou are this task's builder. Do the work, check it against the acceptance, then end your turn with \
                 a handover: what you did, what you checked, what is left. The reviewer reads it.\n",
            );
        }
    }
    let waiting = app.threads.waiting(task_id, seat);
    if !waiting.is_empty() {
        prompt.push_str("\nMessages for your seat:\n");
        for (thread, m) in &waiting {
            prompt.push_str(&format!("- [{thread}] from {}: {}\n", m.from_seat, m.text));
            app.threads.delivered(thread, m.seq, &session.id, now());
        }
    }
    prompt
}

/// The VM of a session said hello: the session runs, and its step's work goes out. Returns whether `cid` was a
/// session's.
pub fn on_hello(app: &App, cid: u64) -> bool {
    let now = now();
    let Some(session) = app
        .sessions
        .by_cid(cid)
        .filter(|s| !s.state.is_final())
        .or_else(|| adopt_orchestrator(app, cid, now))
    else {
        return false;
    };
    if session.state != State::Forking {
        return true;
    }
    let _ = app.sessions.set_state(&session.id, State::Running, now);
    app.bus.publish("session", &app.sessions.get(&session.id));
    // A long-lived session (a planner) waits for its chats' messages and the loop's reports.
    let Some(task_id) = session.task.clone() else {
        return true;
    };
    if session.seat.as_deref() == Some("builder")
        && let Some(task) = app.plaza.tasks.find(&task_id)
        && task.is_open()
        && let Some(agent) = app.agents.get(cid)
    {
        let _ = task.claim(&crate::agents::connected_agent(&agent));
    }
    let prompt = step_prompt(app, &session);
    // A code task's work is a lane in Weft: the builder writes it (task_work opens it, or takes it over from the
    // seat's last session), the reviewer reads its sealed revision (review_work).
    let (prompt, origin) = match app.plaza.tasks.find(&task_id) {
        Some(task) if session.seat.as_deref() == Some("reviewer") => {
            crate::weft_hub::review_work(app, &task, cid, prompt)
        }
        Some(task) => crate::weft_hub::task_work(app, &task, cid, prompt),
        None => (
            prompt,
            Origin {
                task: Some(task_id.clone()),
                ..Origin::default()
            },
        ),
    };
    let work = app.work.enqueue(
        app,
        cid,
        WorkKind::Task,
        format!("task-{task_id}"),
        prompt,
        origin,
    );
    let _ = app.sessions.update(&session.id, |s| s.work = Some(work));
    app.bus.publish("session", &app.sessions.get(&session.id));
    true
}

/// A work item ended on `cid`. When it was a session's step, the seats move on and the session ends. Returns whether
/// the work loop handled it (the scheduler then leaves the task alone).
pub fn on_work_end(app: &App, cid: u64, work: u64, status: WorkStatus, summary: &str) -> bool {
    let Some(session) = app.sessions.by_cid(cid) else {
        return false;
    };
    if session.work != Some(work) {
        return false;
    }
    // A session that is already ending (its VM being retired) has nothing left to move on.
    if !matches!(
        session.state,
        State::Running | State::Paused | State::Compacting
    ) {
        return true;
    }
    let now = now();
    let (Some(task), Some(seat)) = (session.task.clone(), session.seat.clone()) else {
        return false;
    };
    let next = if seat == "reviewer" {
        // The reviewer's step ends with its verdict ([`on_verdict`]); one that ends without one goes again.
        if session.verdict.is_some() {
            None
        } else {
            app.seats.ended(
                &task,
                &seat,
                false,
                true,
                "the review ended without a verdict",
                now,
            )
        }
    } else {
        let passed = status == WorkStatus::Done;
        let crashed = status == WorkStatus::Stopped;
        if passed && let Some(t) = app.plaza.tasks.find(&task) {
            // The builder's lane is sealed while its VM still holds the key: the reviewer reviews that revision.
            crate::weft_hub::seal(app, &task, cid, summary);
            let _ = t.await_review(Some(summary.to_string()));
        }
        app.seats.ended(&task, &seat, passed, crashed, summary, now)
    };
    end_session(app, &session, summary, status == WorkStatus::Failed, now);
    app.threads.output(&task);
    if let Some(next) = next {
        apply(app, &task, next, now);
    }
    true
}

/// The reviewer of `cid`'s session gave its verdict. Returns the reply for the tool, or None when `cid` is not a
/// session's reviewer.
pub fn on_verdict(app: &App, cid: u64, task: &str, pass: bool, notes: &str) -> Option<String> {
    let session = app.sessions.by_cid(cid)?;
    if session.seat.as_deref() != Some("reviewer") || session.task.as_deref() != Some(task) {
        return None;
    }
    let now = now();
    let _ = app.sessions.update(&session.id, |s| s.verdict = Some(pass));
    let next = app.seats.ended(task, "reviewer", pass, false, notes, now)?;
    apply(app, task, next, now);
    Some(if pass {
        format!("Recorded: task {task} passes. End your turn now.")
    } else {
        format!(
            "Recorded: task {task} goes back to its builder with your findings. End your turn now."
        )
    })
}

/// What the seats said, applied to the task.
fn apply(app: &App, task_id: &str, next: Next, now: u64) {
    let task = app.plaza.tasks.find(task_id);
    let project = task
        .as_ref()
        .and_then(|t| t.project().map(str::to_string))
        .unwrap_or_default();
    let chat = task_chat(app, task_id, &project, now);
    match next {
        Next::Ready(_) | Next::Retry(_) => {}
        Next::Back(_) => {
            if let Some(t) = &task {
                let _ = t.rework(None);
            }
            let said = app
                .seats
                .get(task_id, "builder")
                .and_then(|s| s.returned_with)
                .unwrap_or_default();
            notice(
                app,
                &chat,
                &format!("The review sent task {task_id} back to its builder: {said}"),
                now,
            );
        }
        Next::Done => {
            // A code task lands first: its change joins the project's landing queue, and the landing finishes it.
            if join_landing_queue(app, task_id) {
                notice(
                    app,
                    &chat,
                    &format!("Task {task_id} passed review: its change lands next."),
                    now,
                );
                return;
            }
            if let Some(t) = &task {
                let _ = t.finish(FinishOutcome::Done, Some("passed review".into()));
            }
            close_task(app, task_id, now);
            notice(
                app,
                &chat,
                &format!("Task {task_id} is done: it passed review."),
                now,
            );
        }
        Next::Failed(why) => {
            if let Some(t) = &task {
                let _ = t.finish(FinishOutcome::Failed, Some(why.clone()));
            }
            close_task(app, task_id, now);
            notice(
                app,
                &chat,
                &format!("Task {task_id} failed: {why}. The planner decides what follows."),
                now,
            );
            to_planner(
                app,
                &project,
                &format!(
                    "Task {task_id} failed: {why}. Decide what follows: post it again changed, split it, or leave it."
                ),
            );
        }
    }
    app.bus
        .publish("task", &serde_json::json!({ "id": task_id }));
}

/// Puts `task`'s Weft lane on its repo's landing queue (as a passed review graph does). False when the task has no
/// lane, or Weft does not take it: the task is then done on its review.
fn join_landing_queue(app: &App, task: &str) -> bool {
    let Some(change) = app.weft.lane_of(task) else {
        return false;
    };
    let Ok(detail) = app.weft.get(&format!("/changes/{change}")) else {
        return false;
    };
    let rounds = detail["rounds"].as_array().map_or(0, Vec::len);
    match app.weft.hub(
        "queue.add",
        serde_json::json!({ "repo": detail["repo"], "ref": detail["ref"], "change": change, "n": rounds }),
    ) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("apid: {change} did not join its landing queue: {e}");
            false
        }
    }
}

/// `task`'s change landed: the task is done, its sessions end, its seats close and its budget is released.
pub fn landed(app: &App, task: &str, change: &str) {
    let now = now();
    if let Some(t) = app.plaza.tasks.find(task) {
        let _ = t.finish(FinishOutcome::Done, Some(format!("{change} landed")));
    }
    close_task(app, task, now);
    let project = app
        .plaza
        .tasks
        .find(task)
        .and_then(|t| t.project().map(str::to_string))
        .unwrap_or_default();
    notice(
        app,
        &task_chat(app, task, &project, now),
        &format!("Task {task} is done: {change} landed."),
        now,
    );
    // The landing is the project's news too.
    if !project.is_empty() {
        notice(
            app,
            &project_chat(app, &project, now),
            &format!("{change} landed: task {task} is done."),
            now,
        );
    }
    app.bus.publish("task", &serde_json::json!({ "id": task }));
}

/// The project's chat for the loop's notices: its newest, or one made for it.
pub fn project_chat(app: &App, project: &str, now: u64) -> String {
    match app
        .chats
        .list(Some(project), false)
        .into_iter()
        .find(|c| c.kind == crate::chats::Kind::Project)
    {
        Some(c) => c.id,
        None => {
            let title = app
                .projects
                .get(project)
                .map_or_else(|| format!("Project {project}"), |p| p.title);
            app.chats.open_project(project, &title, now).id
        }
    }
}

/// `task`'s change clashed with main on landing: the builder seat goes again with what `prompt` says, as a review that
/// sent it back would.
pub fn clashed(app: &App, task: &str, prompt: &str) {
    let now = now();
    if let Some(next) = app.seats.ended(task, "reviewer", false, false, prompt, now) {
        apply(app, task, next, now);
    }
}

fn close_task(app: &App, task: &str, now: u64) {
    for s in app.sessions.list(&crate::sessions::Filter {
        task: Some(task.to_string()),
        live: true,
        ..Default::default()
    }) {
        end_session(app, &s, "the task ended", false, now);
    }
    app.seats.close(task);
    app.budgets.end_task(task, now);
    for thread in app.threads.of_task(task) {
        app.threads.close(&thread.id, now);
    }
}

/// Ends a session with its handover and retires its VM.
fn end_session(app: &App, session: &Session, handover: &str, failed: bool, now: u64) {
    let _ = app.sessions.update(&session.id, |s| {
        if s.handover.is_none() && !handover.is_empty() {
            s.handover = Some(handover.to_string());
        }
    });
    let _ = app.sessions.set_state(&session.id, State::HandingOver, now);
    let _ = app.sessions.set_state(&session.id, State::Ending, now);
    if let Some(cid) = session.cid {
        retire(app, cid);
    }
    let outcome = if failed {
        "the step failed"
    } else {
        "the step ended"
    };
    let _ = app.sessions.finish(&session.id, outcome, failed, now);
    app.bus.publish("session", &app.sessions.get(&session.id));
}

/// Stops a session by hand (the owner, `POST /v1/sessions/{id}/stop`): it ends and its seat goes again.
pub fn stop(app: &App, id: &str) -> Result<Session, String> {
    let session = app
        .sessions
        .get(id)
        .ok_or_else(|| format!("no session {id}"))?;
    if session.state.is_final() {
        return Ok(session);
    }
    let now = now();
    if let Some(cid) = session.cid {
        app.work.cancel(app, cid, "stopped by the owner");
    }
    end_session(app, &session, "", false, now);
    if let (Some(task), Some(seat)) = (&session.task, &session.seat)
        && let Some(next) = app
            .seats
            .ended(task, seat, false, true, "stopped by the owner", now)
    {
        apply(app, task, next, now);
    }
    app.sessions
        .get(id)
        .ok_or_else(|| format!("no session {id}"))
}

/// The VM of `cid` went away (its node stopped answering, it crashed). Its session fails and its seat goes again.
pub fn on_gone(app: &App, cid: u64) {
    // A session that is handing over or ending retired its own VM: that is not a VM going away.
    if let Some(session) = app.sessions.by_cid(cid)
        && matches!(
            session.state,
            State::Placing | State::Forking | State::Running | State::Paused | State::Compacting
        )
    {
        fail_session(app, &session, "its VM went away", now());
    }
}

/// A project is over (archived or delivered): its open and claimed tasks are withdrawn first (so the seats that end with
/// them do not fail them), its tasks' seats close (their sessions end, their budgets are released), and whatever else
/// still runs for it ends too, its planner included. Its work in flight is cancelled and its VMs stop.
pub fn end_project(app: &App, project: &str, why: &str) {
    let now = now();
    if let Some(found) = app.projects.get(project) {
        for task in &found.tasks {
            if let Some(record) = app.plaza.tasks.find(task)
                && (record.is_open() || record.state() == "claimed")
            {
                let _ = record.withdraw();
            }
        }
        for task in &found.tasks {
            if !app.seats.of_task(task).is_empty() {
                close_task(app, task, now);
            }
        }
    }
    for session in app.sessions.list(&crate::sessions::Filter {
        project: Some(project.to_string()),
        live: true,
        ..Default::default()
    }) {
        if let Some(cid) = session.cid {
            app.work.cancel(app, cid, why);
        }
        end_session(app, &session, "", false, now);
    }
}

/// Retires a session's VM: its lane is cleared and noded stops it.
fn retire(app: &App, cid: u64) {
    app.work.clear(cid);
    app.disk.forget(cid);
    unread_back_to_seats(app, cid);
    if let Some(agent) = app.agents.get(cid)
        && let (Some(run), Some(vm)) = (agent.run.clone(), agent.vm)
    {
        let _ = crate::fleet::noded(
            app,
            &crate::fleet::node_of(app, cid),
            "POST",
            &format!("/v1/runs/{run}/vms/{vm}/stop"),
            Some(&serde_json::json!({})),
        );
    }
    crate::agents::gone(app, cid, "its session ended");
    app.agents.remove(cid);
}

/// The chat of `task` (made on first use).
pub fn task_chat(app: &App, task: &str, project: &str, now: u64) -> String {
    app.chats
        .of_task(task, project, &format!("Task {task}"), now)
        .id
}

/// Posts a notice (what the AIs decided or did) into `chat`.
pub fn notice(app: &App, chat: &str, text: &str, now: u64) {
    let posted = app.chats.post(
        chat,
        Post {
            author: Author::System,
            session: None,
            seat: None,
            kind: MessageKind::Notice,
            text: text.to_string(),
            card: None,
        },
        now,
    );
    if let Some(message) = posted {
        app.bus.publish(
            "chat",
            &serde_json::json!({ "chat": chat, "message": message }),
        );
    }
}

/// Opens a card, and when it is new posts its question in its chat and publishes it. An open card of the same kind and
/// subject is the one kept, and nothing more is posted for it.
pub fn open_card(app: &App, new: NewCard, now: u64) {
    let chat = new.chat.clone();
    let (card, created) = app.cards.open(new, now);
    if !created {
        return;
    }
    let text = match &card.body {
        Some(body) => format!("{}. {body}", card.title),
        None => card.title.clone(),
    };
    if let Some(chat) = chat
        && let Some(message) = app.chats.post(
            &chat,
            Post {
                author: Author::System,
                session: None,
                seat: None,
                kind: MessageKind::Question,
                text,
                card: Some(card.id.clone()),
            },
            now,
        )
    {
        app.bus.publish(
            "chat",
            &serde_json::json!({ "chat": chat, "message": message }),
        );
    }
    app.bus.publish("card", &card);
}

/// A budget limit reached: one card per subject, in the task's chat (or the project's when it is the project's).
pub fn ask_budget(app: &App, short: &Short, task: Option<&str>, project: &str, now: u64) {
    let (subject, title, body) = match short {
        Short::Cap {
            owner_kind,
            owner,
            unit,
            needed,
            left,
        } => (
            subject_of(*owner_kind, owner),
            match owner_kind {
                OwnerKind::Task => format!("Task {owner} reached its budget"),
                OwnerKind::Project if owner == PLAZA => {
                    "The plaza's own budget cannot fund more work".into()
                }
                OwnerKind::Project => format!("Project {owner} cannot fund more work"),
            },
            format!(
                "It needs {} and has {} left ({unit:?}).",
                amount(*needed, *unit),
                amount(*left, *unit)
            ),
        ),
        Short::Monthly {
            project,
            needed,
            left,
        } => (
            format!("monthly:{project}"),
            format!("Project {project}'s planner reached its monthly budget"),
            format!(
                "It needs {} and has {} left this month.",
                amount(*needed, Unit::Usd),
                amount(*left, Unit::Usd)
            ),
        ),
        Short::Paused { owner_kind, owner } => (
            subject_of(*owner_kind, owner),
            format!("{owner_kind:?} {owner} is paused at its budget"),
            "Raise its budget to resume it, or stop it.".to_string(),
        ),
    };
    let chat = match task {
        Some(task) => task_chat(app, task, project, now),
        None => app.chats.main(now).id,
    };
    let (card, created) = app.cards.open(
        NewCard {
            kind: cards::Kind::Budget,
            chat: Some(chat.clone()),
            project: Some(project.to_string()),
            task: task.map(str::to_string),
            session: None,
            subject,
            title: title.clone(),
            body: Some(body.clone()),
            choices: cards::budget_choices(None, "µ$"),
        },
        now,
    );
    if created {
        let posted = app.chats.post(
            &chat,
            Post {
                author: Author::System,
                session: None,
                seat: None,
                kind: MessageKind::Question,
                text: format!("{title}. {body}"),
                card: Some(card.id.clone()),
            },
            now,
        );
        app.bus.publish("card", &card);
        if let Some(message) = posted {
            app.bus.publish(
                "chat",
                &serde_json::json!({ "chat": chat, "message": message }),
            );
        }
    }
}

/// A review seat with no other model to run on asks the owner (W27: cross-model review is mandatory).
fn ask_seat(app: &App, seat: &Seat, project: &str, now: u64) {
    let chat = task_chat(app, &seat.task, project, now);
    let (card, created) = app.cards.open(
        NewCard {
            kind: cards::Kind::Seat,
            chat: Some(chat.clone()),
            project: Some(project.to_string()),
            task: Some(seat.task.clone()),
            session: None,
            subject: format!("seat:{}:{}", seat.task, seat.name),
            title: format!("Task {}'s {} needs another model", seat.task, seat.name),
            body: Some(
                "Every model the project may use already worked on this task, and a review must run on another \
                 model. Allow one more model for the project (its settings), or stop the task."
                    .into(),
            ),
            choices: vec![
                cards::Choice {
                    id: "retry".into(),
                    label: "I allowed another model".into(),
                    detail: Some("look for a model again".into()),
                    takes_amount: false,
                },
                cards::Choice {
                    id: "stop".into(),
                    label: "Stop the task".into(),
                    detail: None,
                    takes_amount: false,
                },
            ],
        },
        now,
    );
    if created {
        let _ = app.chats.post(
            &chat,
            Post {
                author: Author::System,
                session: None,
                seat: None,
                kind: MessageKind::Question,
                text: card.title.clone(),
                card: Some(card.id.clone()),
            },
            now,
        );
        app.bus.publish("card", &card);
    }
}

/// The subject of a budget card: `task:<id>` or `project:<id>` (the budget it raises).
pub fn subject_of(kind: OwnerKind, owner: &str) -> String {
    match kind {
        OwnerKind::Task => format!("task:{owner}"),
        OwnerKind::Project => format!("project:{owner}"),
    }
}

/// The budget a budget card's subject names.
pub fn budget_of_subject(subject: &str) -> Option<(OwnerKind, &str)> {
    match subject.split_once(':')? {
        ("task", id) => Some((OwnerKind::Task, id)),
        ("project", id) => Some((OwnerKind::Project, id)),
        _ => None,
    }
}

fn amount(value: u64, unit: Unit) -> String {
    match unit {
        Unit::Usd => format!("${:.2}", value as f64 / 1_000_000.0),
        Unit::Tokens => format!("{value} tokens"),
    }
}

/// Sessions paused at a budget that `kind`/`owner` covers run again (it was raised or set).
pub fn resume_budget(app: &App, kind: OwnerKind, owner: &str) {
    let now = now();
    let paused = app.sessions.list(&crate::sessions::Filter {
        state: Some(State::Paused),
        ..Default::default()
    });
    for s in paused {
        let covered = match kind {
            OwnerKind::Task => s.task.as_deref() == Some(owner),
            OwnerKind::Project => s.project == owner,
        };
        if covered && s.paused_by == Some(crate::sessions::PausedBy::Budget) {
            let _ = app.sessions.set_state(&s.id, State::Running, now);
            app.bus.publish("session", &app.sessions.get(&s.id));
        }
    }
    let _ = app.budgets.set_paused(kind, owner, false, now);
    // The cards that asked about it are moot now.
    let mut subjects = vec![subject_of(kind, owner)];
    if kind == OwnerKind::Project {
        subjects.push(format!("monthly:{owner}"));
    }
    for subject in subjects {
        for card in app.cards.withdraw(cards::Kind::Budget, &subject, now) {
            app.bus.publish("card", &card);
        }
    }
}

/// Does what an answered card decides; returns it in words.
pub fn on_card_answered(app: &App, card: &cards::Card) -> String {
    let now = now();
    let choice = card.answer.as_deref().unwrap_or("");
    match card.kind {
        cards::Kind::Budget => {
            if let Some(project) = card.subject.strip_prefix("monthly:") {
                return match (choice, card.amount) {
                    ("raise_to", Some(to)) => match app.budgets.get(OwnerKind::Project, project) {
                        Some(b) => {
                            let _ = app.budgets.set_project(
                                project,
                                b.unit,
                                b.cap,
                                b.warn_at_pct,
                                Some(to),
                                b.disk_quota_bytes,
                                now,
                            );
                            resume_budget(app, OwnerKind::Project, project);
                            format!("the planner's monthly budget is now {}", amount(to, b.unit))
                        }
                        None => "no such budget".into(),
                    },
                    _ => "recorded".into(),
                };
            }
            let Some((kind, owner)) = budget_of_subject(&card.subject) else {
                return "recorded".into();
            };
            match choice {
                "raise_to" | "raise_by" => {
                    let (by, to) = if choice == "raise_to" {
                        (None, card.amount)
                    } else {
                        (card.amount, None)
                    };
                    match app.budgets.raise(kind, owner, by, to, now) {
                        Some(b) => {
                            resume_budget(app, kind, owner);
                            format!(
                                "raised to {}",
                                b.cap.map_or("no cap".into(), |c| amount(c, b.unit))
                            )
                        }
                        None => "no such budget".into(),
                    }
                }
                "stop" => match kind {
                    OwnerKind::Task => {
                        stop_task(app, owner, "stopped by the owner at its budget", now);
                        format!("task {owner} stopped")
                    }
                    OwnerKind::Project => {
                        let _ = app.budgets.set_paused(OwnerKind::Project, owner, true, now);
                        format!("project {owner}'s work stays paused")
                    }
                },
                "replan" => {
                    let project = card.project.clone().unwrap_or_default();
                    let what = card.task.clone().unwrap_or_else(|| owner.to_string());
                    to_planner(
                        app,
                        &project,
                        &format!(
                            "The owner asks you to replan {what}: it reached its budget. Split or shrink it."
                        ),
                    );
                    "the planner replans it".into()
                }
                _ => "recorded".into(),
            }
        }
        cards::Kind::Seat => match choice {
            "stop" => {
                if let Some(task) = &card.task {
                    stop_task(
                        app,
                        task,
                        "stopped by the owner: no second model for its review",
                        now,
                    );
                }
                "stopped".into()
            }
            // The seat is still ready: the next tick looks for a model again (and asks again if it finds none).
            _ => "looking for a model again".into(),
        },
        cards::Kind::ToolCall | cards::Kind::Review => crate::approvals::card_answered(app, card),
        cards::Kind::Proposal => crate::agent_proposals::card_answered(app, card),
        cards::Kind::Disk => crate::disk::card_answered(app, card),
        _ => "recorded".into(),
    }
}

/// Stops a task: its sessions end, its seats close, its budget is released, and it is withdrawn.
pub fn stop_task(app: &App, task_id: &str, why: &str, now: u64) {
    if let Some(task) = app.plaza.tasks.find(task_id) {
        let _ = task.withdraw();
    }
    close_task(app, task_id, now);
    let project = app
        .plaza
        .tasks
        .find(task_id)
        .and_then(|t| t.project().map(str::to_string))
        .unwrap_or_default();
    notice(
        app,
        &task_chat(app, task_id, &project, now),
        &format!("Task {task_id}: {why}."),
        now,
    );
}

/// The live long-lived session of `project` (its planner), if one runs.
fn planner_of(app: &App, project: &str) -> Option<Session> {
    app.sessions
        .list(&crate::sessions::Filter {
            project: Some(project.to_string()),
            live: true,
            ..Default::default()
        })
        .into_iter()
        .find(|s| s.kind == Kind::Long && s.cid.is_some())
}

/// The orchestrator's VM: the main orchestrator's long session, else the plaza's orchestrator agent.
fn orchestrator_cid(app: &App) -> Option<u64> {
    if let Some(s) = planner_of(app, "plaza") {
        return s.cid;
    }
    app.agents
        .list()
        .into_iter()
        .find(|a| {
            a.role == crate::settings::ORCHESTRATOR_ROLE
                && a.status != crate::agents::Status::Offline
        })
        .map(|a| a.cid)
}

/// Sends `text` to `project`'s planner (or, until the project has one, to the orchestrator, naming the project).
fn to_planner(app: &App, project: &str, text: &str) {
    let (cid, prompt) = match planner_of(app, project) {
        Some(s) => (s.cid, text.to_string()),
        None => (orchestrator_cid(app), format!("[project {project}] {text}")),
    };
    if let Some(cid) = cid {
        app.work.enqueue(
            app,
            cid,
            WorkKind::Chat,
            format!("chat:project:{project}"),
            prompt,
            Origin {
                owner: true,
                ..Origin::default()
            },
        );
    }
}

/// The owner said `text` in chat `chat`: it goes to the session the chat is with. The main chat is the orchestrator's;
/// a project chat its planner's; a task chat the session filling the task's builder seat now (or, with none, it waits
/// on that seat as a thread message from the owner).
pub fn deliver_owner_message(app: &App, chat: &str, text: &str) {
    let Some(c) = app.chats.get(chat) else {
        return;
    };
    let now = now();
    let thread = format!("chat:{chat}");
    match c.kind {
        crate::chats::Kind::Main => {
            if let Some(cid) = orchestrator_cid(app) {
                app.work.enqueue(
                    app,
                    cid,
                    WorkKind::Chat,
                    thread,
                    text.to_string(),
                    Origin {
                        owner: true,
                        ..Origin::default()
                    },
                );
            } else {
                notice(
                    app,
                    chat,
                    "The orchestrator is not running: it reads this when it starts.",
                    now,
                );
            }
        }
        crate::chats::Kind::Project => {
            let project = c.project.clone().unwrap_or_default();
            let (cid, prompt) = match planner_of(app, &project) {
                Some(s) => (s.cid, text.to_string()),
                None => (
                    orchestrator_cid(app),
                    format!("[project {project}, chat {chat}] {text}"),
                ),
            };
            match cid {
                Some(cid) => {
                    app.work.enqueue(
                        app,
                        cid,
                        WorkKind::Chat,
                        thread,
                        prompt,
                        Origin {
                            owner: true,
                            ..Origin::default()
                        },
                    );
                }
                None => notice(
                    app,
                    chat,
                    "No planner is running for this project yet.",
                    now,
                ),
            }
        }
        crate::chats::Kind::Task => {
            let Some(task) = c.task.clone() else {
                return;
            };
            let filled = app
                .seats
                .of_task(&task)
                .into_iter()
                .find(|s| s.state == seats::SeatState::Filled)
                .and_then(|s| s.session)
                .and_then(|id| app.sessions.get(&id))
                .filter(|s| s.state == State::Running);
            match filled.and_then(|s| s.cid.map(|cid| (s, cid))) {
                Some((_, cid)) => {
                    app.work.enqueue(
                        app,
                        cid,
                        WorkKind::Message,
                        format!("task-{task}"),
                        format!("The owner says: {text}"),
                        Origin {
                            task: Some(task.clone()),
                            owner: true,
                            ..Origin::default()
                        },
                    );
                }
                None => {
                    let _ = app.threads.send(
                        &task,
                        None,
                        "owner",
                        None,
                        "builder",
                        text,
                        Some("from the owner"),
                        now,
                    );
                    notice(
                        app,
                        chat,
                        "No session works on this task now: the next one reads this first.",
                        now,
                    );
                }
            }
        }
    }
}

/// A chat work item (thread `chat:<id>`) ended: its answer goes into the chat. Returns whether it was one.
pub fn on_chat_work_end(app: &App, cid: u64, thread: &str, summary: &str) -> bool {
    let Some(chat) = thread.strip_prefix("chat:") else {
        return false;
    };
    // `chat:project:<id>` items (to_planner) answer in the project's newest chat.
    let chat = match chat.strip_prefix("project:") {
        Some(project) => match app
            .chats
            .list(Some(project), false)
            .into_iter()
            .find(|c| c.kind == crate::chats::Kind::Project)
        {
            Some(c) => c.id,
            None => return true,
        },
        None => chat.to_string(),
    };
    let session = app.sessions.by_cid(cid);
    let posted = app.chats.post(
        &chat,
        Post {
            author: Author::Session,
            session: session.as_ref().map(|s| s.id.clone()),
            seat: session
                .as_ref()
                .and_then(|s| s.seat.clone())
                .or_else(|| Some("orchestrator".into())),
            kind: MessageKind::Message,
            text: summary.to_string(),
            card: None,
        },
        now(),
    );
    if let Some(message) = posted {
        app.bus.publish(
            "chat",
            &serde_json::json!({ "chat": chat, "message": message }),
        );
    }
    true
}

// ── model calls against budgets (Engine 0.5) ─────────────────────────────────────────────────────────────────────

/// A model call held on its budget: given by [`before_call`], returned to [`after_call`]. `session` is `None` for a VM
/// that runs no session, whose calls are held on the plaza's own budget.
pub struct CallHold {
    hold: crate::budgets::Hold,
    session: Option<String>,
    model: String,
}

/// What `input` and `output` tokens of `model` cost, in both units (micro-dollars at the model's price, and tokens).
fn call_cost(app: &App, model: &str, tokens: crate::prices::Tokens) -> Amount {
    let usd = app
        .prices
        .lookup(model, &app.settings.get().prices)
        .map_or(0.0, |priced| {
            crate::prices::estimate(&priced.price, &tokens)
        });
    Amount::of(
        (usd * 1_000_000.0).ceil() as u64,
        tokens.input + tokens.output + tokens.cache_read + tokens.cache_write,
    )
}

/// Before a model call of `cid`: the call's worst case (the request's size and its answer's cap, at the model's price)
/// is held on the session's task, or on its project for a long-lived session; a VM that runs no session (one the owner
/// spawned, a fork) spends from the plaza's own budget, so no call goes unmetered. A budget that cannot take it pauses
/// the session and asks the owner (one card per budget); the `Err` is the harness's `paused:` answer
/// ([`apz_proto::wire::MODEL_PAUSED`]), which makes the turn wait and ask again.
pub fn before_call(
    app: &App,
    cid: u64,
    model: &str,
    request: &apz_proto::chat::ChatRequest,
) -> Result<Option<CallHold>, String> {
    let session = app.sessions.by_cid(cid);
    let now = now();
    let size = serde_json::to_string(&request.messages).map_or(0, |t| t.len())
        + serde_json::to_string(&request.tools).map_or(0, |t| t.len());
    // About three bytes a token, rounded up: the hold is a ceiling, and the settle charges what the call really cost.
    let worst = call_cost(
        app,
        model,
        crate::prices::Tokens {
            input: (size as u64).div_ceil(3),
            output: match request.max_tokens {
                0 => UNCAPPED_ANSWER_TOKENS,
                cap => u64::from(cap),
            },
            cache_read: 0,
            cache_write: 0,
        },
    );
    let (task, project) = match &session {
        Some(session) => (session.task.as_deref(), session.project.as_str()),
        None => (None, PLAZA),
    };
    match app.budgets.hold(
        task,
        project,
        worst,
        session.as_ref().map(|s| s.id.as_str()),
        Some(model),
        now,
    ) {
        Ok(hold) => {
            if let Some(session) = &session
                && session.state == State::Paused
                && session.paused_by == Some(PausedBy::Budget)
            {
                let _ = app.sessions.set_state(&session.id, State::Running, now);
                app.bus.publish("session", &app.sessions.get(&session.id));
            }
            Ok(Some(CallHold {
                hold,
                session: session.map(|s| s.id),
                model: model.to_string(),
            }))
        }
        Err(short) => {
            if let Some(session) = &session
                && session.state != State::Paused
            {
                let _ = app.sessions.pause(&session.id, PausedBy::Budget, now);
                app.bus.publish("session", &app.sessions.get(&session.id));
            }
            ask_budget(app, &short, task, project, now);
            Err(format!(
                "{}{}",
                apz_proto::wire::MODEL_PAUSED,
                short_sentence(&short)
            ))
        }
    }
}

/// After the call: what it really cost is charged (and the hold released), its usage is added to the session, and a
/// budget it took past its warning share says so in the task's chat. A failed call (`usage` none) charges nothing.
pub fn after_call(app: &App, cid: u64, held: CallHold, usage: Option<&apz_proto::chat::Usage>) {
    let now = now();
    let Some(usage) = usage else {
        app.budgets.release(&held.hold, now);
        return;
    };
    let tokens = crate::prices::Tokens {
        input: usage.input_tokens,
        output: usage.output_tokens,
        cache_read: usage.cache_read_tokens,
        cache_write: usage.cache_write_tokens,
    };
    let actual = call_cost(app, &held.model, tokens);
    let _ = app.sessions.add_usage(
        cid,
        &crate::sessions::Usage {
            tokens_in: usage.input_tokens,
            tokens_out: usage.output_tokens,
            tokens_cached: usage.cache_read_tokens,
            cost_micros: actual.micros,
            model_calls: 1,
            tool_calls: 0,
        },
    );
    let notices = app.budgets.settle(
        &held.hold,
        actual,
        held.session.as_deref(),
        Some(&held.model),
        now,
    );
    for Notice::Warn {
        owner_kind,
        owner,
        spent,
        cap,
    } in notices
    {
        let unit = app
            .budgets
            .get(owner_kind, &owner)
            .map_or(Unit::Usd, |b| b.unit);
        let (task, project) = match owner_kind {
            OwnerKind::Task => (
                Some(owner.clone()),
                app.plaza
                    .tasks
                    .find(&owner)
                    .and_then(|t| t.project().map(str::to_string))
                    .unwrap_or_default(),
            ),
            OwnerKind::Project => (None, owner.clone()),
        };
        let chat = match &task {
            Some(task) => task_chat(app, task, &project, now),
            None => app.chats.main(now).id,
        };
        notice(
            app,
            &chat,
            &format!(
                "{owner_kind:?} {owner} has spent {} of its {} budget.",
                amount(spent, unit),
                amount(cap, unit)
            ),
            now,
        );
    }
}

/// Why a call was held, in a sentence for the harness's log.
fn short_sentence(short: &Short) -> String {
    match short {
        Short::Cap {
            owner_kind,
            owner,
            unit,
            needed,
            left,
        } => format!(
            "{owner_kind:?} {owner} has {} left and the call may cost {}; the owner was asked to raise it",
            amount(*left, *unit),
            amount(*needed, *unit)
        ),
        Short::Monthly { project, .. } => {
            format!("project {project} reached its monthly budget; the owner was asked to raise it")
        }
        Short::Paused { owner_kind, owner } => {
            format!("{owner_kind:?} {owner} is paused at its budget")
        }
    }
}

// ── seat messages (Engine 0.8) ───────────────────────────────────────────────────────────────────────────────────

/// A seat message handed to a running session as a work item: (the VM, the thread, the message's seq, the session).
type Delivery = (u64, String, u64, String);

/// The seat messages handed to running sessions and not read yet, by work item.
static DELIVERIES: Mutex<Option<HashMap<u64, Delivery>>> = Mutex::new(None);

fn deliveries<R>(f: impl FnOnce(&mut HashMap<u64, Delivery>) -> R) -> R {
    let mut guard = DELIVERIES.lock().unwrap_or_else(PoisonError::into_inner);
    f(guard.get_or_insert_with(HashMap::new))
}

/// `plaza_message_seat`: the session of `cid` sends `text` to a seat (`reviewer`, `builder`: a seat of its own task;
/// `T12/reviewer`: another task's; `planner`: its project's planner). The session filling the seat now gets it as a
/// work item; with none, it waits on the seat and is the first thing the seat's next session reads. `thread` continues
/// a thread; a new one gets `subject`. A thread that loops (more than [`crate::threads::LOOP_LIMIT`] messages with
/// no output) is reported to the planner. Returns what happened, for the tool's answer.
pub fn message_seat(
    app: &App,
    cid: u64,
    to: &str,
    text: &str,
    thread: Option<&str>,
    subject: Option<&str>,
    close: bool,
) -> Result<String, String> {
    let session = app.sessions.by_cid(cid).ok_or(
        "only a session's seat sends seat messages: use plaza_send_message to reach an agent",
    )?;
    let from_seat = session.seat.clone().unwrap_or_else(|| session.role.clone());
    let from = match &session.task {
        Some(task) => format!("{task}/{from_seat}"),
        None => from_seat.clone(),
    };
    let now = now();
    match crate::threads::address(to, session.task.as_deref()).ok_or_else(|| {
        format!("`{to}` is not a seat: write `reviewer`, `T12/builder` or `planner`")
    })? {
        crate::threads::Address::Planner => {
            to_planner(app, &session.project, &format!("[from {from}] {text}"));
            Ok("sent to the project's planner".into())
        }
        crate::threads::Address::Seat { task, seat } => {
            let target = app
                .seats
                .get(&task, &seat)
                .ok_or_else(|| format!("task {task} has no seat {seat}"))?;
            let sent = app.threads.send(
                &task,
                thread,
                &from,
                Some(&session.id),
                &seat,
                text,
                subject,
                now,
            )?;
            // An answer that settles the question closes its thread (the loop count starts again).
            if close {
                app.threads.close(&sent.thread, now);
            }
            if sent.looped {
                to_planner(
                    app,
                    &session.project,
                    &format!(
                        "Thread {} of task {task} looped: more than {} messages between its seats without an output. \
                         Read it (GET /v1/threads/{}) and settle the question or stop the task.",
                        sent.thread,
                        crate::threads::LOOP_LIMIT,
                        sent.thread
                    ),
                );
            }
            let running = target
                .session
                .and_then(|id| app.sessions.get(&id))
                .filter(|s| s.state == State::Running)
                .and_then(|s| s.cid.map(|cid| (s.id, cid)));
            match running {
                Some((id, to_cid)) => {
                    let work = app.work.enqueue(
                        app,
                        to_cid,
                        WorkKind::Message,
                        format!("task-{task}"),
                        format!(
                            "[thread {}] from {from}: {text}\n(Answer with plaza_message_seat to \"{from}\", thread \"{}\".)",
                            sent.thread, sent.thread
                        ),
                        Origin {
                            task: Some(task.clone()),
                            ..Origin::default()
                        },
                    );
                    app.threads.pend(&sent.thread, sent.message.seq);
                    deliveries(|d| {
                        d.insert(work, (to_cid, sent.thread.clone(), sent.message.seq, id));
                    });
                    Ok(format!(
                        "sent to {task}/{seat} in thread {}: its session reads it next",
                        sent.thread
                    ))
                }
                None => Ok(format!(
                    "sent in thread {}: it waits for the next session of {task}/{seat}",
                    sent.thread
                )),
            }
        }
    }
}

/// A work item ended: a seat message it carried was read.
pub fn on_item_done(app: &App, work: u64) {
    if let Some((_, thread, seq, session)) = deliveries(|d| d.remove(&work)) {
        app.threads.delivered(&thread, seq, &session, now());
    }
}

/// The VM of `cid` is retired: the seat messages handed to it and not read wait on their seats again.
fn unread_back_to_seats(app: &App, cid: u64) {
    let unread: Vec<Delivery> = deliveries(|d| {
        let works: Vec<u64> = d
            .iter()
            .filter(|(_, (to, ..))| *to == cid)
            .map(|(work, _)| *work)
            .collect();
        works.into_iter().filter_map(|w| d.remove(&w)).collect()
    });
    for (_, thread, seq, _) in unread {
        app.threads.unpend(&thread, seq);
    }
}

// ── long-lived sessions: the main orchestrator and the projects' planners (Engine 0.3, 0.10) ─────────────────────

/// The "project" of the main orchestrator's long-lived session: the plaza as a whole.
pub const PLAZA: &str = "plaza";
/// The role of a project's planner, when the plaza has it.
pub const PLANNER_ROLE: &str = "planner";
/// How long a project waits after its planner's session failed before it starts another.
pub const PLANNER_BACKOFF_MS: u64 = 10 * 60_000;

/// An orchestrator VM that says hello with no session becomes the main orchestrator's long-lived session (the plaza's
/// fleet starts it, and starts it again after a crash): the main chat is with it, and its calls are the plaza's. A
/// session the previous VM left live ends. None for any other VM.
fn adopt_orchestrator(app: &App, cid: u64, now: u64) -> Option<Session> {
    let agent = app.agents.get(cid)?;
    if agent.role != crate::settings::ORCHESTRATOR_ROLE {
        return None;
    }
    for stale in live_long(app, PLAZA) {
        let _ = app
            .sessions
            .finish(&stale.id, "another orchestrator VM took over", false, now);
    }
    let session = app.sessions.create(
        NewSession {
            project: PLAZA.into(),
            task: None,
            seat: None,
            kind: Kind::Long,
            parent: None,
            role: agent.role.clone(),
            engine: ENGINE.into(),
            model: Some(crate::agents::resolved_model(app, &agent).0),
            attempt: 1,
        },
        now,
    );
    let _ = app.sessions.update(&session.id, |s| s.cid = Some(cid));
    let _ = app.sessions.set_state(&session.id, State::Placing, now);
    let _ = app.sessions.set_state(&session.id, State::Forking, now);
    app.sessions.get(&session.id)
}

/// The live long-lived sessions of `project`.
fn live_long(app: &App, project: &str) -> Vec<Session> {
    app.sessions
        .list(&crate::sessions::Filter {
            project: Some(project.to_string()),
            live: true,
            ..Default::default()
        })
        .into_iter()
        .filter(|s| s.kind == Kind::Long)
        .collect()
}

/// Every active project has its planner, a long-lived session, when the plaza has the planner role (in maintenance the
/// planner is the one who sweeps it, `maintenance.rs`): one that has none gets one, unless its last one failed within
/// [`PLANNER_BACKOFF_MS`].
fn ensure_planners(app: &App, now: u64) {
    if !crate::roles_custom::exists(app, PLANNER_ROLE) {
        return;
    }
    for project in app.projects.list() {
        if !project_active(app, &project.id) || !live_long(app, &project.id).is_empty() {
            continue;
        }
        let failed_lately = app
            .sessions
            .list(&crate::sessions::Filter {
                project: Some(project.id.clone()),
                state: Some(State::Failed),
                ..Default::default()
            })
            .into_iter()
            .any(|s| {
                s.kind == Kind::Long
                    && now.saturating_sub(s.ended_ms.unwrap_or(0)) < PLANNER_BACKOFF_MS
            });
        if !failed_lately {
            start_planner(app, &project.id, now);
        }
    }
}

/// Starts `project`'s planner: a long-lived session on a VM of its own, on the project's model for the role.
fn start_planner(app: &App, project: &str, now: u64) {
    let model = candidates(app, project, PLANNER_ROLE).into_iter().next();
    let session = app.sessions.create(
        NewSession {
            project: project.to_string(),
            task: None,
            seat: None,
            kind: Kind::Long,
            parent: None,
            role: PLANNER_ROLE.into(),
            engine: ENGINE.into(),
            model: model.clone(),
            attempt: 1,
        },
        now,
    );
    let _ = app.sessions.set_state(&session.id, State::Placing, now);
    match crate::agents::spawn_agents(app, PLANNER_ROLE, 1, model.as_deref(), None, Some(project)) {
        Ok(started) => match started["cids"].get(0).and_then(serde_json::Value::as_u64) {
            Some(cid) => {
                let node = started["node"].as_str().map(str::to_string);
                let _ = app.sessions.update(&session.id, |s| {
                    s.cid = Some(cid);
                    s.node = node;
                });
                let _ = app.sessions.set_state(&session.id, State::Forking, now);
                app.bus.publish("session", &app.sessions.get(&session.id));
            }
            None => fail_session(app, &session, "the node started no VM", now),
        },
        Err(why) => fail_session(app, &session, &why, now),
    }
}
