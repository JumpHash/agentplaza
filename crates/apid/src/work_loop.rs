//! The work loop (Engine 0.3, 0.4): tasks run as sessions filling seats.
//!
//! Every tick (the scheduler's, every 2 s):
//! 1. a task that is ready to run (open, every `after` link done, its project active, no seats yet) gets its recipe's
//!    seats (`seats.rs`: the built-in change recipe) and reserves its budget from its project (`budgets.rs`), when its
//!    project's team has room (its maximum, PLAN 7.1), the fleet has room and its budget rate does; a project that
//!    cannot reserve it with nothing else running asks the owner with a `budget` card and the task waits;
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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, PoisonError};

use apz_plaza::tasks::FinishOutcome;
use apz_proto::wire::{WorkKind, WorkStatus};
use serde_json::Value;

use crate::agents::Agent;
use crate::budgets::{Amount, Notice, OwnerKind, Short, Unit};
use crate::cards::{self, NewCard};
use crate::chats::{Author, MessageKind, Post};
use crate::evals::choose;
use crate::journal;
use crate::placement;
use crate::project_settings;
use crate::recipes::model::Model;
use crate::recipes::{self, Move, Report};
use crate::review_graph;
use crate::scheduler::Origin;
use crate::seats::{self, Next, Seat};
use crate::sessions::{Filter, Kind, NewSession, PausedBy, Session, State};
use crate::settings::{Tier, tier_of};
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
    crate::curator::tick(app, now);
    crate::verifier::tick(app, now);
    assign_tasks(app, now);
    for seat in app.seats.ready() {
        // The plan step waits for the owner's answers to its spec's questions (planning.rs).
        if crate::planning::waits_for_owner(app, &seat.task, &seat.step) {
            continue;
        }
        start_seat(app, &seat, now);
    }
    reap(app, now);
    crate::disk::tick(app, now);
    crate::intake::tick(app, now);
}

/// Whether project `id` takes work: it exists, is not paused or archived, and is in a stage that runs tasks, or in
/// maintenance (its planner sweeps it, and the tasks the owner picks from its proposals run).
pub fn project_active(app: &App, id: &str) -> bool {
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

/// A seat's step ended. A crashed session is the seat's own business; otherwise the task's recipe run settles the step
/// and its moves (what starts, what rewinds, what ends) are the seats' next.
fn step_ended(
    app: &App,
    task: &str,
    seat: &str,
    passed: bool,
    crashed: bool,
    said: &str,
    now: u64,
) -> Option<Next> {
    if crashed {
        return app.seats.ended(task, seat, passed, crashed, said, now, &[]);
    }
    let step = app.seats.get(task, seat)?.step;
    // A planning step holds when its file is on the lane or its plan is posted (planning.rs).
    let passed = crate::planning::step_holds(app, task, &step, passed, now);
    let report = Report {
        findings: (step == "verify").then(|| review_graph::seat_findings(app, task)),
        ..Report::verdict(passed)
    };
    let moves = app.recipe_runs.settle_task(task, &step, &report, now);
    review_graph::seat_settled(app, task, &moves);
    // A rewind sends the builder the findings the verifier confirmed (the review's own record), not the verifier's words.
    let said = if moves.iter().any(|m| matches!(m, Move::Rewind(_))) {
        review_graph::seat_said(app, task)
    } else {
        said.to_string()
    };
    app.seats
        .ended(task, seat, passed, crashed, &said, now, &moves)
}

/// The tasks of project `project` that run now: those that hold seats, from their seats' opening until they end (a
/// task that a session has claimed is no longer open, and still runs). They are the project's team (PLAN 7.1).
fn running_tasks(app: &App, project: &str) -> Vec<String> {
    app.plaza
        .tasks
        .list()
        .iter()
        .filter(|t| t.project() == Some(project) && !app.seats.of_task(t.id()).is_empty())
        .map(|t| t.id().to_string())
        .collect()
}

/// Reserves `reserve` for `task` from its project as the budget rate allows (PLAN 7.1): the expected spend of the
/// project's running tasks, kept under what is left. A running task expects its reserve, which it holds from the
/// project when it has a budget of its own; one without holds nothing, so its default reserve is counted here.
/// Refused with the project's `Short` when the rest does not fit.
fn reserve_budget(
    app: &App,
    task: &str,
    project: &str,
    reserve: u64,
    now: u64,
) -> Result<(), Short> {
    let unreserved: u64 = running_tasks(app, project)
        .iter()
        .filter(|running| app.budgets.get(OwnerKind::Task, running).is_none())
        .map(|running| reserve_for(app, running, project))
        .sum();
    if let Some(budget) = app.budgets.get(OwnerKind::Project, project)
        && let Some(left) = budget.left()
        && left.saturating_sub(unreserved) < reserve
    {
        return Err(Short::Cap {
            owner_kind: OwnerKind::Project,
            owner: budget.owner.clone(),
            unit: budget.unit,
            needed: reserve,
            left: left.saturating_sub(unreserved),
        });
    }
    app.budgets.start_task(task, project, reserve, now)
}

/// Fills the seats of every ready task (PLAN 0.4: `assign_tasks` fills seats) up to its project's team (PLAN 7.1): a
/// task that is ready to run gets its recipe's seats and reserves its budget; its seats get sessions on the loop's next
/// steps. A task waits while its project runs its maximum of tasks, while every node that reports is full, and while its
/// reserve does not fit the budget's rate ([`reserve_budget`]).
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
        let running = running_tasks(app, &project).len();
        if running >= project_settings::team_max(app, &project) {
            continue;
        }
        // The seat a task opens with is the builder's, so the fleet is asked for a builder's room.
        if placement::fleet_full(app, "builder") {
            continue;
        }
        let reserve = reserve_for(app, task.id(), &project);
        if let Err(short) = reserve_budget(app, task.id(), &project, reserve, now) {
            // The running tasks hand their spend back as they end, so a task that waits for them is not short.
            if running > 0 && matches!(short, Short::Cap { .. }) {
                continue;
            }
            ask_budget(app, &short, Some(task.id()), &project, now);
            continue;
        }
        // The recipe the task runs: the one planning gave it (planning.rs), else the change recipe.
        let recipe_id = crate::planning::recipe_id_of(app, task.id());
        let (recipe, source) = task_recipe(app, &project, &recipe_id);
        if let Err(why) = app.seats.open(task.id(), &recipe, now) {
            app.budgets.end_task(task.id(), now);
            notice(
                app,
                &task_chat(app, task.id(), &project, now),
                &format!("Task {} cannot start: {why}.", task.id()),
                now,
            );
            continue;
        }
        app.recipe_runs.start(
            &recipe,
            source,
            "task",
            Some(task.id()),
            Some(&project),
            None,
            now,
        );
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

/// The planner's `team_set {max?, target?}` (PLAN 7.1): sets the team of the planner's project, the most of its tasks
/// that run at once and the team it aims for. Only a planner is offered it (tools.jhrl). A lower maximum starts no new
/// task until the team is back under it; the tasks running go on.
pub fn team_set(app: &App, agent: &Agent, input: &Value) -> Result<String, String> {
    let project = crate::projects::project_of(app, agent.cid)
        .ok_or("team_set sets the team of a project, and you work on none")?;
    let max = team_number(input, "max")?;
    let target = team_number(input, "target")?;
    if max.is_none() && target.is_none() {
        return Err("invalid input: give max, target or both".into());
    }
    let mut settings = app.project_settings.get(&project);
    if max.is_some() {
        settings.team_max = max;
    }
    if target.is_some() {
        settings.team_target = target;
    }
    project_settings::check(app, &settings)?;
    app.project_settings
        .set(&project, settings)
        .map_err(|e| format!("the team was not saved: {e}"))?;
    if let Some(record) = app.projects.get(&project) {
        app.bus.publish("project", &record);
    }
    Ok(format!(
        "team of {project}: at most {} tasks at once, aiming for {}",
        project_settings::team_max(app, &project),
        project_settings::team_target(app, &project)
    ))
}

/// A team size from a tool's input: a whole number, or none when the key is absent.
fn team_number(input: &Value, key: &str) -> Result<Option<u32>, String> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| format!("invalid input: {key:?} must be a whole number")),
    }
}

/// The recipe a task of `project` runs (`id`): the project's own copy at main (`.plaza/recipes/`), else the newest
/// version the owner saved, else the built-in. A copy that cannot be read or checked is named in the log, and the next
/// one runs. Returns the recipe and where it came from.
fn task_recipe(app: &App, project: &str, id: &str) -> (recipes::Recipe, &'static str) {
    match project_copy(app, project, id) {
        Ok(Some(recipe)) => return (recipe, "project"),
        Ok(None) => {}
        Err(why) => eprintln!("apid: recipe {id} of {project}: {why}"),
    }
    let recipe = app
        .recipe_runs
        .recipe(id, None)
        .expect("a built-in recipe is always there");
    let source = if recipes::model::builtin_named(id).as_ref() == Some(&recipe) {
        "built_in"
    } else {
        "owner"
    };
    (recipe, source)
}

/// The project's copy of recipe `id` in its repo at main, when it keeps one.
fn project_copy(app: &App, project: &str, id: &str) -> Result<Option<recipes::Recipe>, String> {
    let Some(repo) = app.projects.get(project).and_then(|p| p.repo) else {
        return Ok(None);
    };
    let detail = app.weft.get(&format!("/repos/{repo}"))?;
    let rev = detail["refs"]["main"]
        .as_str()
        .ok_or_else(|| format!("{repo} has no main"))?;
    let listing = app.weft.get(&format!("/revisions/{rev}/files"))?;
    let client = app.weft.client("hub")?;
    recipes::model::in_listing(id, &listing, |path| {
        client.get_bytes(&format!("/revisions/{rev}/file?path={path}"))
    })
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
    // A differ seat never runs on the builder's model (the cross-model rule, 8.2); the other seats' models are avoided
    // where another is allowed, so a sceptic and the verifier take other models where the project has them.
    let author: HashSet<String> = app
        .seats
        .of_task(&seat.task)
        .into_iter()
        .filter(|s| s.role == "builder")
        .filter_map(|s| s.last_model)
        .collect();
    let others: HashSet<String> = app
        .seats
        .models_to_avoid(&seat.task, &seat.name)
        .into_iter()
        .collect();
    if let Some(chosen) = app
        .project_settings
        .get(project)
        .role_models
        .get(&seat.role)
        .filter(|m| !m.is_empty() && !author.contains(*m) && !others.contains(*m))
    {
        return Some(chosen.clone());
    }
    let usable: Vec<&String> = all.iter().filter(|m| !author.contains(*m)).collect();
    let other_providers: HashSet<&str> = others.iter().map(|m| provider_of(m)).collect();
    usable
        .iter()
        .find(|m| !others.contains(**m) && !other_providers.contains(provider_of(m)))
        .or_else(|| usable.iter().find(|m| !others.contains(**m)))
        .or_else(|| usable.first())
        .map(|m| (*m).clone())
}

/// A builder's change is sealed: a large or risky change (the review step's `risk`) gets its extra sceptics before the
/// round's reviews start (8.2). The walk's items and the seats grow together.
fn size_review(app: &App, task: &str, now: u64) {
    let Some((base, Some(risk))) = app.recipe_runs.sizing_of_task(task, "review") else {
        return;
    };
    let Some(change) = app.weft.lane_of(task) else {
        return;
    };
    let rev = review_graph::latest_revision(app, &change);
    let Ok(diff) = app.weft.get(&format!("/revisions/{rev}/diff")) else {
        return;
    };
    let mut lines = 0u32;
    let mut risky = false;
    for file in diff["files"].as_array().into_iter().flatten() {
        let path = file["path"].as_str().unwrap_or("");
        if risk.paths.iter().any(|g| weft::derived::glob(g, path)) {
            risky = true;
        }
        for line in file["unified"].as_str().unwrap_or("").lines() {
            if (line.starts_with('+') && !line.starts_with("+++"))
                || (line.starts_with('-') && !line.starts_with("---"))
            {
                lines += 1;
            }
        }
    }
    let count = if risky || lines > risk.lines {
        base + risk.extra
    } else {
        base
    };
    app.recipe_runs.set_items_task(task, "review", count, now);
    app.seats.grow(task, "review", count, now);
}

/// The seat roles that review a change: the reviewers (the sceptics) and the verifier.
pub fn review_role(role: &str) -> bool {
    role == "reviewer" || role == "verifier"
}

/// The model a step names when the evals choose it (`model: eval`).
const EVAL_MODEL: &str = "eval";
/// The engine a step names when the evals choose it (`engine: any`).
const ANY_ENGINE: &str = "any";

/// What a seat's session runs on, and how that was chosen: the rule and the reason go on the step's eval record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub engine: String,
    pub model: String,
    pub rule: &'static str,
    pub reason: String,
}

/// The engines a seat's session can run on. Only the plaza's harness runs a seat today: the vendor engines run through
/// the relay, not as seats, so `engine: any` chooses among these.
fn seat_engines() -> Vec<String> {
    vec![ENGINE.to_string()]
}

/// The models a step with `model: eval` may take: the policy's list for the seat's tier, in its order, without repeats.
/// A model the policy does not list for the tier is never picked by the evals.
fn tier_models(app: &App, role: &str) -> Vec<String> {
    let policy = app.settings.get().model_policy;
    let list = match tier_of(role) {
        Tier::Orchestrator => policy.orchestrator,
        Tier::Worker => policy.worker,
    };
    let mut out: Vec<String> = Vec::new();
    for model in list {
        if !out.contains(&model) {
            out.push(model);
        }
    }
    out
}

/// The eval policy the settings name: the runs a score needs before it is trusted, and the explore share.
fn eval_policy(app: &App) -> choose::Policy {
    let settings = app.settings.get();
    choose::Policy {
        min_samples: u64::from(settings.eval_min_samples),
        explore_pct: settings.eval_explore_pct,
    }
}

/// What a step may still spend, in micro-dollars: its share of its task's dollar cap (the project's, when the task has
/// none), less what the step already spent on the task, and not more than the owner has left. None when the owner has no
/// dollar cap: a token budget is not checked against the candidates' dollars.
fn step_budget_left(app: &App, project: &str, seat: &Seat, share: f64) -> Option<u64> {
    let owner = [
        app.budgets.get(OwnerKind::Task, &seat.task),
        app.budgets.get(OwnerKind::Project, project),
    ]
    .into_iter()
    .flatten()
    .find(|b| b.cap.is_some())?;
    if owner.unit != Unit::Usd {
        return None;
    }
    let share_of_cap = (owner.cap? as f64 * share) as u64;
    let spent = app.evals.step_spend(project, &seat.task, &seat.step);
    Some(
        owner
            .left()
            .unwrap_or(u64::MAX)
            .min(share_of_cap.saturating_sub(spent)),
    )
}

/// The model for `seat`'s session, as its step's recipe says it: a named model as it is, a model list's first model the
/// project allows, or the seat's own choice (`any`, `differ`). None when there is none.
fn named_model(
    app: &App,
    project: &str,
    seat: &Seat,
    model: &Model,
) -> Option<(String, &'static str)> {
    match model {
        Model::Name(name) if name == "any" || name == "differ" => {
            pick_model(app, project, seat).map(|m| (m, choose::SEAT))
        }
        Model::Name(pinned) => Some((pinned.clone(), choose::FIXED)),
        Model::List(list) => first_listed(list, &candidates(app, project, &seat.role))
            .map(|m| (m, choose::LISTED))
            .or_else(|| pick_model(app, project, seat).map(|m| (m, choose::SEAT))),
    }
}

/// The engine and model for `seat`'s session, as its step's recipe says them (13.2). `model: eval` takes the best model
/// by the evals of the step among the policy's models for the seat's tier, and `engine: any` the best engine; both
/// within the step's budget, with the exploration share and the minimum sample of the settings. Each choice keeps its
/// rule and reason. None when no candidate is left: the seat asks the owner.
fn pick_step_model(app: &App, project: &str, seat: &Seat) -> Option<Choice> {
    let spec = app.recipe_runs.step_spec(&seat.task, &seat.step);
    let model = spec
        .as_ref()
        .map_or_else(Model::default, |s| s.model.clone());
    let any_engine = spec
        .as_ref()
        .is_some_and(|s| s.engine.as_deref() == Some(ANY_ENGINE));
    let eval = matches!(&model, Model::Name(name) if name == EVAL_MODEL);
    if !eval && !any_engine {
        let (model, rule) = named_model(app, project, seat, &model)?;
        return Some(Choice {
            engine: ENGINE.into(),
            model,
            rule,
            reason: String::new(),
        });
    }
    let policy = eval_policy(app);
    let share = spec.as_ref().map_or(1.0, |s| s.budget_share);
    let budget = step_budget_left(app, project, seat, share);
    let scores = app.evals.scores();
    // The run's number: the seat's task, seat and attempt, so the same seat run always makes the same explore choice.
    let run = choose::key_of(&format!(
        "{}/{}#{}",
        seat.task,
        seat.name,
        seat.attempts + 1
    ));
    let mut reasons = Vec::new();
    let engine = if any_engine {
        let pick = choose::engine(&scores, &seat.step, &seat_engines(), budget, policy, run)?;
        reasons.push(format!("engine: {}", pick.reason));
        pick.key
    } else {
        ENGINE.to_string()
    };
    let (model, rule) = if eval {
        let pick = choose::model(
            &scores,
            &seat.step,
            &engine,
            &tier_models(app, &seat.role),
            budget,
            policy,
            run,
        )?;
        reasons.push(format!("model: {}", pick.reason));
        (pick.key, pick.rule)
    } else {
        named_model(app, project, seat, &model)?
    };
    Some(Choice {
        engine,
        model,
        rule,
        reason: reasons.join("; "),
    })
}

/// The first model of `list` that `allowed` (the project's models for the role, best first) has.
pub fn first_listed(list: &[String], allowed: &[String]) -> Option<String> {
    list.iter().find(|m| allowed.contains(m)).cloned()
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
    // A project past its disk quota starts no fork: the seat stays ready, and the disk card asks the owner (10.5).
    if crate::disk::admit_fork(app, &project, now).is_err() {
        return;
    }
    // Nor one whose declared pools have no node with room: the seat waits for one (16.7).
    if !crate::placement::has_room_for_fork(app, &seat.role, &project) {
        return;
    }
    let Some(choice) = pick_step_model(app, &project, seat) else {
        ask_seat(app, seat, &project, now);
        return;
    };
    let model = choice.model.clone();
    if review_role(&seat.role) {
        review_graph::open_round(app, &seat.task);
    }
    let session = app.sessions.create(
        NewSession {
            project: project.clone(),
            task: Some(seat.task.clone()),
            seat: Some(seat.name.clone()),
            kind: Kind::Step,
            parent: None,
            role: seat.role.clone(),
            engine: choice.engine.clone(),
            model: Some(model.clone()),
            attempt: seat.attempts + 1,
        },
        now,
    );
    app.seats
        .fill(&seat.task, &seat.name, &session.id, Some(&model), now);
    app.recipe_runs
        .task_step_started(&seat.task, &seat.step, now);
    let recipe_version = app
        .recipe_runs
        .recipe(&seat.recipe, None)
        .map_or(1, |recipe| recipe.version);
    app.evals.begin(
        &session.id,
        &seat.recipe,
        recipe_version,
        &seat.step,
        seat.round,
    );
    app.evals.decided(&session.id, choice.rule, &choice.reason);
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
            if review_role(&seat.role) {
                review_graph::seat_started(app, &seat.task, &seat.name, cid, &model, &session.id);
            }
            let _ = app.sessions.set_state(&session.id, State::Forking, now);
            app.bus.publish("session", &app.sessions.get(&session.id));
        }
        Err(why) => fail_session(app, &session, &why, now),
    }
}

/// A session that could not start, or whose VM went away: it fails and its seat goes again (or fails for good).
pub fn fail_session(app: &App, session: &Session, why: &str, now: u64) {
    finish_session(app, &session.id, why, true, now);
    if let Some(cid) = session.cid {
        retire(app, cid);
    }
    if let (Some(task), Some(seat)) = (&session.task, &session.seat)
        && let Some(next) = step_ended(app, task, seat, false, true, why, now)
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

/// The step's prompt for a session that just started: the brief, the journal's notes, the handover the seat starts from
/// (the builder's for a reviewer, the seat's own last one otherwise), the seat's instructions and the messages waiting.
fn step_prompt(app: &App, session: &Session) -> String {
    let Some(task_id) = &session.task else {
        return String::new();
    };
    let Some(task) = app.plaza.tasks.find(task_id) else {
        return String::new();
    };
    let seat = session.seat.as_deref().unwrap_or("builder");
    let mut prompt = journal::brief_text(&task);
    if let Some(journal) = app.journal.get(task_id)
        && !journal.notes.is_empty()
    {
        prompt.push_str(&format!(
            "\nThe task's journal:\n{}",
            journal::notes_text(&journal.notes)
        ));
    }
    // The newest handover of the seat this one starts from (the builder's for a reviewer), by an earlier session.
    let source = if review_role(&session.role) {
        "builder"
    } else {
        seat
    };
    let handover = app
        .sessions
        .list(&crate::sessions::Filter {
            task: Some(task_id.clone()),
            ..Default::default()
        })
        .into_iter()
        .filter(|s| s.id != session.id && s.seat.as_deref() == Some(source))
        .filter_map(|s| s.handover.map(|h| (s.created_ms, h)))
        .max_by_key(|(created, _)| *created)
        .map(|(_, handover)| handover);
    match seat {
        _ if review_role(&session.role) => {
            if let Some(handover) = handover {
                prompt.push_str(&format!("\nThe builder's handover:\n{handover}\n"));
            }
            if session.role == "verifier" {
                prompt.push_str(
                    "\nYou are this task's verifier, on a model the builder and the reviewers did not use where one is \
                     free. Prove each finding below with a test, a run or the exact lines, or drop it with its reason \
                     (finding_verify), then call plaza_verdict with pass true.\n",
                );
            } else {
                prompt.push_str(
                    "\nYou are one of this task's sceptic reviewers, on a model other than the builder's. Try to break \
                     the change: check it against the acceptance, run what proves it, and look for what the builder \
                     missed.\n",
                );
            }
            prompt.push_str(&review_graph::seat_brief(app, task_id, &session.role));
        }
        _ => {
            if let Some(handover) = handover {
                prompt.push_str(&format!("\nYour last handover on this task:\n{handover}\n"));
            }
            if let Some(back) = app.seats.get(task_id, seat).and_then(|s| s.returned_with) {
                prompt.push_str(&format!("\nThe review sent the change back:\n{back}\n"));
            }
            prompt.push_str(
                "\nYou are this task's builder. Do the work and check it against the acceptance. Write each decision \
                 with its reason, each dead end with why it failed and each open question with journal_note as you \
                 go. Then end with handover_write: where the task stands, what is done, what is next, what to check \
                 first. The reviewer reads it. If nothing is left for another session, end your turn with the line \
                 TASK DONE instead.\n",
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
    // A project's curator has no task: its work is the run it was started for (curator.rs).
    if session.seat.as_deref() == Some(crate::curator::SEAT) && session.task.is_none() {
        crate::curator::start_work(app, &session, cid);
        return true;
    }
    if session.seat.as_deref() == Some(crate::verifier::SEAT) && session.task.is_none() {
        crate::verifier::start_work(app, &session, cid);
        return true;
    }
    // A long-lived session (a planner) waits for its chats' messages and the loop's reports.
    let Some(task_id) = session.task.clone() else {
        return true;
    };
    // The builder claims the task, and so does a spec-and-plan task's planner (it is finished when its plan is posted).
    if (session.seat.as_deref() == Some("builder") || crate::planning::is_plan(app, &task_id))
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
        Some(task) if review_role(&session.role) => {
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
    // The helpers the work item started end with it.
    end_helpers(app, &session.id, now());
    // A session that is already ending (its VM being retired) has nothing left to move on.
    if !matches!(
        session.state,
        State::Running | State::Paused | State::Compacting
    ) {
        return true;
    }
    let now = now();
    if session.seat.as_deref() == Some(crate::curator::SEAT) && session.task.is_none() {
        // A curator's run ends with its answer (memory_land) or without one: its tasks wait for the next run.
        end_session(app, &session, summary, status == WorkStatus::Failed, now);
        return true;
    }
    if session.seat.as_deref() == Some(crate::verifier::SEAT) && session.task.is_none() {
        // A verifier's run ends with its answer (memory_verify) or without one: the flagged entries wait for the next run.
        end_session(app, &session, summary, status == WorkStatus::Failed, now);
        return true;
    }
    let (Some(task), Some(seat)) = (session.task.clone(), session.seat.clone()) else {
        return false;
    };
    let next = if review_role(&session.role) {
        // A review seat's step ends with its verdict ([`on_verdict`]); one that ends without one goes again.
        if session.verdict.is_some() {
            None
        } else {
            step_ended(
                app,
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
        if passed
            && !crate::planning::is_plan(app, &task)
            && let Some(t) = app.plaza.tasks.find(&task)
        {
            // The builder's lane is sealed while its VM still holds the key: the reviewer reviews that revision.
            crate::weft_hub::seal(app, &task, cid, summary);
            size_review(app, &task, now);
            let _ = t.await_review(Some(summary.to_string()));
        }
        step_ended(app, &task, &seat, passed, crashed, summary, now)
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
    if !review_role(&session.role) || session.task.as_deref() != Some(task) {
        return None;
    }
    let seat = session.seat.clone()?;
    if session.role == "verifier" && pass {
        let open = review_graph::pending_findings(app, task);
        if !open.is_empty() {
            return Some(format!(
                "Not recorded: rule on every finding first (finding_verify): {}.",
                open.join(", ")
            ));
        }
    }
    // A contest the verifier holds holds the walk until the planner (or the owner) rules (8.5): the verdict is kept on
    // the session, and [`resume_held_review`] settles it.
    if session.role == "verifier" && pass && review_graph::contested_held(app, task) {
        let _ = app.sessions.update(&session.id, |s| s.verdict = Some(pass));
        return Some(
            "Held: the verifier holds a contested finding, and the review goes on once it is ruled. End your turn now."
                .to_string(),
        );
    }
    let now = now();
    let _ = app.sessions.update(&session.id, |s| s.verdict = Some(pass));
    let next = step_ended(app, task, &seat, pass, false, notes, now);
    if let Some(next) = next.clone() {
        apply(app, task, next, now);
    }
    Some(match next {
        Some(Next::Back(_)) => format!(
            "Recorded: task {task} goes back to its builder with the confirmed findings. End your turn now."
        ),
        Some(Next::Done) => format!("Recorded: task {task} passes. End your turn now."),
        _ => format!("Recorded: seat {seat} reported. End your turn now."),
    })
}

/// A held review resumes once its contested findings are ruled: the verifier's held verdict settles on the walk, so a
/// fix rewinds the builder and an accept or a drop lets the walk go on.
pub fn resume_held_review(app: &App, task: &str) {
    if review_graph::contested_held(app, task) {
        return;
    }
    let Some(seat) = app.seats.get(task, "verifier") else {
        return;
    };
    if seat.state != seats::SeatState::Filled {
        return;
    }
    let Some(session) = seat.session.as_deref().and_then(|id| app.sessions.get(id)) else {
        return;
    };
    if session.verdict != Some(true) {
        return;
    }
    let now = now();
    if let Some(next) = step_ended(app, task, "verifier", true, false, "", now) {
        apply(app, task, next, now);
    }
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
        // A spec-and-plan task is done when its plan is posted: nothing lands from it (planning.rs).
        Next::Done if crate::planning::is_plan(app, task_id) => {
            crate::planning::finish(app, task_id, now);
        }
        Next::Done => {
            // The project's land hooks check the change before it joins the landing queue: a finding of a block-mode
            // hook sends the task back to its builder, with the finding (PLAN 14.3).
            let refusals = landing_refusals(app, task_id, &project);
            if !refusals.is_empty() {
                let said = format!(
                    "the landing hooks refused the change: {}",
                    refusals.join("; ")
                );
                if let Some(t) = &task {
                    let _ = t.rework(Some(said.clone()));
                }
                notice(
                    app,
                    &chat,
                    &format!("Task {task_id} went back to its builder: {said}."),
                    now,
                );
                return;
            }
            // The reviewers and the verifier ran on models other than the builder's (8.2): with one the same, the owner rules.
            if let Some(gap) = review_graph::seat_gap(app, task_id) {
                ask_cross_model(app, task_id, &gap, now);
                return;
            }
            admit(app, task_id, now);
        }
        Next::Failed(why) => {
            if let Some(t) = &task {
                let _ = t.finish(FinishOutcome::Failed, Some(why.clone()));
            }
            close_task(app, task_id, now);
            if let Err(err) = app.evals.replay_finished(task_id, false, now) {
                eprintln!("apid: evals: the replay of {task_id} was not scored: {err}");
            }
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
/// The findings the project's land hooks refuse the task's change on: the block-mode checks of its `land` hooks over
/// the lines the change's diff adds. None when the project has no such hook, the task has no change, or Weft does not
/// answer (the landing then goes on as it did before the hooks).
fn landing_refusals(app: &App, task: &str, project: &str) -> Vec<String> {
    if project.is_empty() {
        return Vec::new();
    }
    let hooks = crate::project_settings::effective_hooks(&app.project_settings.get(project));
    if !hooks
        .iter()
        .any(|hook| hook.on == apz_proto::hooks::On::Land)
    {
        return Vec::new();
    }
    let Some(change) = app.weft.lane_of(task) else {
        return Vec::new();
    };
    let Ok(rev) = crate::project_memory::landed_revision(app, &change) else {
        return Vec::new();
    };
    let Ok(diff) = app.weft.get(&format!("/revisions/{rev}/diff")) else {
        return Vec::new();
    };
    let unified: Vec<&str> = diff["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|file| file["unified"].as_str())
        .collect();
    crate::project_settings::land_refusals(&hooks, &unified.join("\n"))
}

/// The review passed and the change is admitted: it joins its project's landing queue, or the task is done at once
/// when it has no lane. Its owner-approved admission (a cross-model card) and the review's pass both come here.
pub fn admit(app: &App, task_id: &str, now: u64) {
    let task = app.plaza.tasks.find(task_id);
    let project = task
        .as_ref()
        .and_then(|t| t.project().map(str::to_string))
        .unwrap_or_default();
    let chat = task_chat(app, task_id, &project, now);
    // A code task lands first: its change joins the project's landing queue, and the landing finishes it.
    if join_landing_queue(app, task_id) {
        notice(
            app,
            &chat,
            &format!("Task {task_id} passed review: its change lands next."),
            now,
        );
    } else {
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
    app.bus
        .publish("task", &serde_json::json!({ "id": task_id }));
}

/// The owner's card for a reviewer on the builder's own model: the task waits for the owner to admit it or fail it.
fn ask_cross_model(app: &App, task: &str, gap: &str, now: u64) {
    let Some(record) = app.plaza.tasks.find(task) else {
        return;
    };
    let view = record.view();
    let worker = view.agent_cid.unwrap_or(0);
    let detail = serde_json::json!({
        "gate": "cross_model",
        "task": task,
        "cid": worker,
        "worker": app.agents.get(worker).map(|a| a.label()).unwrap_or_default(),
        "goal": record.goal(),
        "summary": gap,
        "verdict": gap,
        "needs_you": gap,
        "readings": [],
        "choices": ["pass", "fail"],
    });
    let item = app
        .approvals
        .submit(worker, "review", &format!("review of task {task}"), detail);
    crate::approvals::announce(app, &item);
    notice(
        app,
        &task_chat(app, task, record.project().unwrap_or_default(), now),
        &format!("Task {task} waits for the owner: {gap}."),
        now,
    );
}

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
    if let Err(err) = app.evals.landed(task, true) {
        eprintln!("apid: evals: the landing of {task} was not recorded: {err}");
    }
    if let Err(err) = app.evals.replay_finished(task, true, now) {
        eprintln!("apid: evals: the replay of {task} was not scored: {err}");
    }
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
    // An item the task came from hears of its landing (intake's write-back).
    crate::intake::landed(app, task, change);
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
    if let Some(next) = step_ended(app, task, "reviewer", false, false, prompt, now) {
        apply(app, task, next, now);
    }
}

pub(crate) fn close_task(app: &App, task: &str, now: u64) {
    // The task's journal waits for its project's curator (curator.rs).
    if let Some(project) = app
        .plaza
        .tasks
        .find(task)
        .and_then(|t| t.project().map(str::to_string))
    {
        crate::curator::requested(app, &project, task);
    }
    for s in app.sessions.list(&crate::sessions::Filter {
        task: Some(task.to_string()),
        live: true,
        ..Default::default()
    }) {
        end_session(app, &s, "the task ended", false, now);
    }
    app.recipe_runs.cancel_task(task, now);
    app.seats.close(task);
    app.budgets.end_task(task, now);
    for thread in app.threads.of_task(task) {
        app.threads.close(&thread.id, now);
    }
}

/// Ends a session with its handover and retires its VM.
pub fn end_session(app: &App, session: &Session, handover: &str, failed: bool, now: u64) {
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
    finish_session(app, &session.id, outcome, failed, now);
    app.bus.publish("session", &app.sessions.get(&session.id));
}

/// Finishes a session and writes its eval record, the first time it ends.
fn finish_session(app: &App, id: &str, outcome: &str, failed: bool, now: u64) {
    let was_final = app.sessions.get(id).is_none_or(|s| s.state.is_final());
    if let Ok(ended) = app.sessions.finish(id, outcome, failed, now)
        && !was_final
    {
        crate::evals::ended(app, &ended);
        // A project's curator session is the run of its `distill` recipe: the session's end settles it.
        if ended.seat.as_deref() == Some(crate::curator::SEAT) && ended.task.is_none() {
            app.recipe_runs.finish_subject(
                "curator",
                &ended.project,
                &Report::verdict(!failed),
                outcome,
                false,
                now,
            );
        }
    }
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
        && let Some(next) = step_ended(app, task, seat, false, true, "stopped by the owner", now)
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

/// Whether a step session in `state` hands over when its machine has no other machine in its pool with room
/// (pure, plan 17.2): a step that is placed, running or paused does; one that is already handing over or
/// ending is left to end on its own.
pub fn hands_over_for_pool(state: &State) -> bool {
    matches!(
        state,
        State::Placing | State::Forking | State::Running | State::Paused | State::Compacting
    )
}

/// The step session of agent `cid` hands over when its node is stopping and no machine in its pool has room
/// (plan 17.2): it ends with a handover that the next session of its seat starts from, its VM is retired, and
/// its seat goes again in a new session, which is placed on any machine with room. True when it did; an agent
/// that runs no step is left to the mover to wait.
pub fn hand_over_for_pool(app: &App, cid: u64) -> bool {
    let Some(session) = app.sessions.by_cid(cid) else {
        return false;
    };
    let (Some(task), Some(seat)) = (session.task.clone(), session.seat.clone()) else {
        return false;
    };
    if !hands_over_for_pool(&session.state) {
        return false;
    }
    let now = now();
    let why = "its machine is stopping and no other machine in its pool has room; the step starts again in a new session";
    end_session(app, &session, why, false, now);
    if let Some(next) = step_ended(app, &task, &seat, false, true, why, now) {
        apply(app, &task, next, now);
    }
    true
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
            attachments: Vec::new(),
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
                attachments: Vec::new(),
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

/// The unit a budget card's amounts are in: the short budget's own; a monthly or paused limit is the planner's dollars.
fn card_unit(short: &Short) -> Unit {
    match short {
        Short::Cap { unit, .. } => *unit,
        Short::Monthly { .. } | Short::Paused { .. } => Unit::Usd,
    }
}

/// The subject, title and body of the card a budget limit asks the owner about. Every amount is in the unit the
/// budget counts in: dollars for `usd`, tokens for `tokens` (never micro-dollars).
pub fn budget_words(short: &Short) -> (String, String, String) {
    match short {
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
                "It needs {} and has {} left.",
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
    }
}

/// A budget limit reached: one card per subject, in the task's chat (or the project's when it is the project's).
pub fn ask_budget(app: &App, short: &Short, task: Option<&str>, project: &str, now: u64) {
    let (subject, title, body) = budget_words(short);
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
            choices: cards::budget_choices(None, card_unit(short)),
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
                attachments: Vec::new(),
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
                attachments: Vec::new(),
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

/// An amount in its budget's unit, for a person: `$1.25` from micro-dollars, `12,400 tokens` for tokens.
pub fn amount(value: u64, unit: Unit) -> String {
    match unit {
        Unit::Usd => format!("${:.2}", value as f64 / 1_000_000.0),
        Unit::Tokens => format!("{} tokens", thousands(value)),
    }
}

/// A whole number with a comma every three digits: `12400` is `12,400`.
fn thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
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
        cards::Kind::Engine => crate::engine_watch::card_answered(app, card),
        cards::Kind::Connector if card.subject.starts_with("tool:") => {
            crate::tool_life::card_answered(app, card)
        }
        cards::Kind::Connector => crate::connectors::card_answered(app, card),
        cards::Kind::Promote | cards::Kind::Tool => crate::tool_life::card_answered(app, card),
        cards::Kind::Disk => crate::disk::card_answered(app, card),
        cards::Kind::Allowance => crate::allowances::card_answered(app, card),
        cards::Kind::Start => crate::drafts::answered(app, card),
        cards::Kind::Memory => crate::curator::card_answered(app, card),
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
/// The planner session's VM of `project`, if it has a live one.
pub fn planner_cid(app: &App, project: &str) -> Option<u64> {
    planner_of(app, project).and_then(|s| s.cid)
}

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
pub fn to_planner(app: &App, project: &str, text: &str) {
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
                let work = app.work.enqueue(
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
                record_work(app, chat, work);
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
                    let work = app.work.enqueue(
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
                    record_work(app, chat, work);
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

/// The VM a chat's owner messages are with now: the main chat's orchestrator, a project chat's planner (else the
/// orchestrator, until the project has one). None for a task's chat, whose messages go to the seat that fills its task.
pub fn chat_cid(app: &App, chat: &crate::chats::Chat) -> Option<u64> {
    match chat.kind {
        crate::chats::Kind::Main => orchestrator_cid(app),
        crate::chats::Kind::Project => chat
            .project
            .as_deref()
            .and_then(|project| planner_of(app, project))
            .and_then(|s| s.cid)
            .or_else(|| orchestrator_cid(app)),
        crate::chats::Kind::Task => None,
    }
}

/// Records that work item `work` answers chat `chat`'s owner message (the chat's turns, for its context).
fn record_work(app: &App, chat: &str, work: u64) {
    let _ = app.chats.update(chat, |c| {
        c.works.push(work);
        Ok(())
    });
}

/// Tells the session of `cid` a plaza notice on chat `chat`'s thread (a project started, a task ended): the chat shows
/// `shown`, and the answer the session gives is posted there ([`on_chat_work_end`]).
pub fn tell_chat(app: &App, chat: &str, cid: u64, shown: &str, prompt: &str) {
    notice(app, chat, shown, now());
    if app.agents.get(cid).is_none() {
        eprintln!("apid: a notice for {chat} dropped: agent {cid} is gone");
        return;
    }
    app.work.enqueue(
        app,
        cid,
        WorkKind::Chat,
        format!("chat:{chat}"),
        prompt.to_string(),
        Origin::default(),
    );
}

/// What a chat shows of a plaza notice's prompt: its first line, without the `[plaza]` mark.
pub fn shown_line(prompt: &str) -> &str {
    let first = prompt.lines().next().unwrap_or_default();
    first.strip_prefix("[plaza] ").unwrap_or(first)
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
            attachments: Vec::new(),
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
    let size = serde_json::to_string(&request.messages).map_or(0, |t| t.len())
        + serde_json::to_string(&request.tools).map_or(0, |t| t.len());
    hold_call(app, cid, model, size as u64, request.max_tokens)
}

/// [`before_call`] for a call whose request the caller sized itself: `size` is the request's size in bytes and
/// `max_tokens` the answer cap it names (0 when it names none). The LLM proxy holds its calls this way, and a VM that
/// runs no session holds on the plaza's budget the same way.
pub fn hold_call(
    app: &App,
    cid: u64,
    model: &str,
    size: u64,
    max_tokens: u32,
) -> Result<Option<CallHold>, String> {
    let session = app.sessions.by_cid(cid);
    let now = now();
    // The hold is a ceiling: the settle charges what the call really cost.
    let worst = call_cost(app, model, worst_case(size, max_tokens));
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

/// A call's worst case in tokens, from its request's size in bytes and the answer cap it names (0 when it names none):
/// about three bytes a token, rounded up, and the answer its cap, or [`UNCAPPED_ANSWER_TOKENS`] when it has none.
pub fn worst_case(size: u64, max_tokens: u32) -> crate::prices::Tokens {
    crate::prices::Tokens {
        input: size.div_ceil(3),
        output: match max_tokens {
            0 => UNCAPPED_ANSWER_TOKENS,
            cap => u64::from(cap),
        },
        cache_read: 0,
        cache_write: 0,
    }
}

/// Releases a call's hold without charging it: the call failed, or returned no usage.
pub fn release_call(app: &App, held: CallHold) {
    app.budgets.release(&held.hold, now());
}

/// After the call: what it really cost is charged (and the hold released), its usage is added to the session, and a
/// budget it took past its warning share says so in the task's chat. A failed call (`usage` none) charges nothing.
pub fn after_call(
    app: &App,
    cid: u64,
    held: CallHold,
    usage: Option<&apz_proto::chat::Usage>,
    helper: Option<&str>,
) {
    let Some(usage) = usage else {
        release_call(app, held);
        return;
    };
    settle_call(
        app,
        cid,
        held,
        crate::prices::Tokens {
            input: usage.input_tokens,
            output: usage.output_tokens,
            cache_read: usage.cache_read_tokens,
            cache_write: usage.cache_write_tokens,
        },
        None,
        helper,
    );
}

/// [`after_call`] for a call whose usage the caller parsed itself (the LLM proxy's, streams included): `reported_usd` is
/// the cost the provider reported for the call when it did, and the model's price settles it when it did not. A
/// helper's call (`helper`, its session's id) is recorded on the helper's session.
pub fn settle_call(
    app: &App,
    cid: u64,
    held: CallHold,
    tokens: crate::prices::Tokens,
    reported_usd: Option<f64>,
    helper: Option<&str>,
) {
    let now = now();
    let all_tokens = tokens.input + tokens.output + tokens.cache_read + tokens.cache_write;
    let actual = match reported_usd {
        Some(usd) => Amount::of((usd * 1_000_000.0).ceil() as u64, all_tokens),
        None => call_cost(app, &held.model, tokens),
    };
    let usage = crate::sessions::Usage {
        tokens_in: tokens.input,
        tokens_out: tokens.output,
        tokens_cached: tokens.cache_read,
        cost_micros: actual.micros,
        model_calls: 1,
        tool_calls: 0,
    };
    // A helper's call is recorded on the helper's session; the budget is the session's either way.
    match helper {
        Some(id) => {
            let _ = app.sessions.add_usage_to(id, &usage);
        }
        None => {
            let _ = app.sessions.add_usage(cid, &usage);
        }
    }
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

/// The session of helper `helper` that the VM `cid` runs (a `Kind::Helper` session under the VM's session, on its task
/// and project), created at the helper's first model call. None when the VM runs no session: its calls spend from the
/// plaza's budget and are not recorded on a helper.
pub fn helper_session(app: &App, cid: u64, helper: u64) -> Option<String> {
    static HELPERS: Mutex<BTreeMap<(String, u64), String>> = Mutex::new(BTreeMap::new());
    let parent = app.sessions.by_cid(cid)?;
    let key = (parent.id.clone(), helper);
    if let Some(id) = HELPERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return Some(id.clone());
    }
    let now = now();
    let session = app.sessions.create(
        NewSession {
            project: parent.project.clone(),
            task: parent.task.clone(),
            seat: None,
            kind: Kind::Helper,
            parent: Some(parent.id.clone()),
            role: parent.role.clone(),
            engine: parent.engine.clone(),
            model: parent.model.clone(),
            attempt: 1,
        },
        now,
    );
    // A helper runs at once: it is placed and forked with its VM's.
    for to in [State::Placing, State::Forking, State::Running] {
        let _ = app.sessions.set_state(&session.id, to, now);
    }
    app.bus.publish("session", &app.sessions.get(&session.id));
    HELPERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key, session.id.clone());
    Some(session.id)
}

/// Ends the helper sessions of `parent` that still run: their work item ended.
fn end_helpers(app: &App, parent: &str, now: u64) {
    let live = app.sessions.list(&Filter {
        live: true,
        ..Filter::default()
    });
    for helper in live
        .into_iter()
        .filter(|s| s.kind == Kind::Helper && s.parent.as_deref() == Some(parent))
    {
        let _ = app
            .sessions
            .finish(&helper.id, "its work item ended", false, now);
        app.bus.publish("session", &app.sessions.get(&helper.id));
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
/// The seat thread `thread`, as the session of `cid` reads it: its messages, for a thread of the session's own
/// task only. The thread as JSON.
pub fn thread_read(app: &App, cid: u64, thread: &str) -> Result<String, String> {
    let session = app.sessions.by_cid(cid).ok_or(
        "only a session's seat reads seat threads: use plaza_send_message to reach an agent",
    )?;
    let found = own_thread(app, session.task.as_deref(), thread)?;
    serde_json::to_string(&found).map_err(|err| err.to_string())
}

/// Answers the seat thread `thread` for the session of `cid`: to the seat its last message from another seat
/// came from, in the same thread (see [`message_seat`]).
pub fn thread_answer(
    app: &App,
    cid: u64,
    thread: &str,
    text: &str,
    close: bool,
) -> Result<String, String> {
    let session = app.sessions.by_cid(cid).ok_or(
        "only a session's seat answers seat threads: use plaza_send_message to reach an agent",
    )?;
    let found = own_thread(app, session.task.as_deref(), thread)?;
    let from_seat = session.seat.clone().unwrap_or_else(|| session.role.clone());
    let me = match &session.task {
        Some(task) => format!("{task}/{from_seat}"),
        None => from_seat,
    };
    let to = answer_seat(&found.messages, &me)
        .ok_or_else(|| format!("nobody in thread {thread} waits for an answer from you"))?;
    message_seat(app, cid, &to, text, Some(thread), None, close)
}

/// The seat an answer to a thread goes to: the seat that last wrote in it, other than `me`.
fn answer_seat(messages: &[crate::threads::Message], me: &str) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|message| message.from_seat != me)
        .map(|message| message.from_seat.clone())
}

/// The thread `id`, when it belongs to `task` (the session's own task).
fn own_thread(app: &App, task: Option<&str>, id: &str) -> Result<crate::threads::Thread, String> {
    let thread = app
        .threads
        .get(id)
        .ok_or_else(|| format!("there is no thread {id}"))?;
    if task != Some(thread.task.as_str()) {
        return Err(format!("thread {id} is not of your task"));
    }
    Ok(thread)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_team_size_is_a_whole_number_and_an_absent_one_is_none() {
        let input = serde_json::json!({"max": 8, "target": null, "bad": "many", "neg": -1});
        assert_eq!(team_number(&input, "max"), Ok(Some(8)));
        assert_eq!(team_number(&input, "target"), Ok(None));
        assert_eq!(team_number(&input, "missing"), Ok(None));
        assert!(team_number(&input, "bad").is_err());
        assert!(team_number(&input, "neg").is_err());
    }

    #[test]
    fn a_step_in_flight_hands_over_when_its_pool_has_no_room() {
        for state in [
            State::Placing,
            State::Forking,
            State::Running,
            State::Paused,
            State::Compacting,
        ] {
            assert!(hands_over_for_pool(&state), "{state:?} is in flight");
        }
        for state in [
            State::Queued,
            State::HandingOver,
            State::Ending,
            State::Ended,
            State::Failed,
        ] {
            assert!(
                !hands_over_for_pool(&state),
                "{state:?} is not left to hand over"
            );
        }
    }

    #[test]
    fn a_model_list_takes_its_first_allowed_model() {
        let allowed = vec!["a/fast".to_string(), "b/strong".to_string()];
        let listed = ["c/none", "b/strong", "a/fast"].map(String::from);
        assert_eq!(first_listed(&listed, &allowed), Some("b/strong".into()));
        assert_eq!(first_listed(&["c/none".to_string()], &allowed), None);
    }

    #[test]
    fn a_call_is_held_at_its_request_size_in_tokens_and_the_answer_cap_it_names() {
        let worst = worst_case(300, 500);
        assert_eq!((worst.input, worst.output), (100, 500));
        // About three bytes a token, rounded up; a call that names no cap holds the largest default answer.
        assert_eq!(worst_case(301, 0).input, 101);
        assert_eq!(worst_case(0, 0).output, UNCAPPED_ANSWER_TOKENS);
    }

    #[test]
    fn a_budget_card_names_its_amounts_in_the_budgets_unit() {
        let tokens = Short::Cap {
            owner_kind: OwnerKind::Task,
            owner: "t1".into(),
            unit: Unit::Tokens,
            needed: 12_400,
            left: 300,
        };
        let (subject, _, body) = budget_words(&tokens);
        assert_eq!(subject, "task:t1");
        assert_eq!(body, "It needs 12,400 tokens and has 300 tokens left.");
        let dollars = Short::Cap {
            owner_kind: OwnerKind::Project,
            owner: "pr1".into(),
            unit: Unit::Usd,
            needed: 1_250_000,
            left: 0,
        };
        assert_eq!(
            budget_words(&dollars).2,
            "It needs $1.25 and has $0.00 left."
        );
        assert_eq!(card_unit(&tokens), Unit::Tokens);
        let choices = cards::budget_choices(Some(2_000_000), Unit::Tokens);
        assert_eq!(choices[0].label, "Raise by the estimate (2,000,000 tokens)");
        assert!(
            !format!("{body}{}", choices[0].label).contains("µ$"),
            "no card says micro-dollars for a token budget"
        );
    }
}

#[cfg(test)]
mod thread_answer_tests {
    use super::answer_seat;
    use crate::threads::Message;

    fn said(from: &str, seq: u64) -> Message {
        Message {
            seq,
            from_seat: from.to_string(),
            from_session: None,
            to_seat: "x".to_string(),
            text: format!("message {seq}"),
            at_ms: seq,
            delivered_session: None,
            delivered_ms: None,
        }
    }

    #[test]
    fn an_answer_goes_to_the_seat_that_last_wrote_and_not_to_oneself() {
        let thread = [
            said("T1/reviewer", 1),
            said("T1/builder", 2),
            said("T1/reviewer", 3),
        ];
        assert_eq!(
            answer_seat(&thread, "T1/builder").as_deref(),
            Some("T1/reviewer")
        );
        // The builder's own question, unanswered: nobody but itself has written, so nothing to answer.
        assert_eq!(answer_seat(&[said("T1/builder", 1)], "T1/builder"), None);
        assert_eq!(answer_seat(&[], "T1/builder"), None);
    }
}
