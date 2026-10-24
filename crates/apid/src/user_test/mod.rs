//! The `test:user` step (PLAN 19): a recipe step that runs scenarios of a change against a target, as a person would.
//!
//! Three roles share it (models.rs): the planner writes the scenarios (the spec's acceptance, the past confirmed
//! findings as regressions, an exploratory budget within the step's config) and reads each failure; the driver picks
//! actions, escalating below its threshold; the checker looks at checkpoints and at screens with no tree. The loop
//! (run.rs) runs each scenario on a fresh target, with the built-in accessibility scenario last.
//!
//! A failure is a finding on the change's review run (`review_graph/user.rs`): a scenario's failure on the `fea` node,
//! a timing over its budget on the `prf` node. The verifier re-runs the failing scenario on a fresh target: a failure that
//! holds again is confirmed (with the evidence of the re-run as its proof); one that does not is dropped as flaky. The
//! planner reads the failure first: a failure it judges not a defect of the product is dropped without a re-run.
//!
//! A run is started by the owner's fire route (`triggers/routes.rs`) and queued here; the scheduler's tick runs it on its
//! own thread (checks.rs is the same shape). The target is the one the step's config names (`opener`): the stand-in
//! (target.rs, in-process data), or a web page or a terminal program placed in the lane's test fork (fork.rs): the browser
//! and the program run in the fork, and the hub only drives them over the fork's exec channel and its port streams. The
//! desktop target (desktop.rs) is a station's screen, leased for the run and driven through the station tools; the
//! simulator and device targets of 8.4–8.5 implement the same trait.

pub mod android;
#[cfg(test)]
mod android_test;
pub mod desk;
pub mod desktop;
#[cfg(test)]
mod desktop_targets_test;
#[cfg(test)]
mod desktop_test;
pub mod farm;
pub mod fork;
pub mod macos;
pub mod models;
pub mod run;
pub mod scenario;
pub mod simulator;
pub mod target;
#[cfg(test)]
mod targets_test;
pub mod tui;
pub mod web;

use std::cell::RefCell;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::base_builds;
use crate::base_recipe;
use crate::base_vm::{Fork, Live};
use crate::placement;
use crate::previews::life::publish_port;
use crate::previews::proxy::LOCAL_INBOUND;
use crate::project_memory;
use crate::project_stack::{self, TargetKind};
use crate::recipes::graph::StepState;
use crate::recipes::model::{FarmTest, USER_TEST, UserTest};
use crate::recipes::runs::CheckRow;
use crate::recipes::{Move, Report};
use crate::review_graph::user::by;
use crate::review_graph::{Finding, Verdict};
use crate::state::App;
use apz_blobs::Blobs;
use desktop::{FORK_STATION, Forked, Gui, Hub, LINUX};
use fork::{AGENT_PORT, Channel, DEVTOOLS_PORT, agent_command, browser_command, port_dial};
use models::{GatewayModels, Judgement, Look, Models, Pick, PickRequest, PlanRequest, Roles, Tier};
use run::{Ctx, Failure, Timed, run_scenario};
use scenario::{Origin, Scenario, accessibility};
use target::{Screenshot, Target};
use tui::ForkTui;
use web::Web;

/// The inputs of one execution: the change's symbol, its acceptance, the regression scenarios and the exploratory budget.
pub struct Input<'a> {
    pub symbol: &'a str,
    pub acceptance: &'a [String],
    pub regressions: &'a [Scenario],
    pub exploratory: u32,
}

/// The verifier's ruling on a failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ruling {
    /// The re-run failed again: the proof is what the re-run showed, with its evidence.
    Confirmed { proof: String },
    /// The planner read it as not a defect, or the re-run did not fail again: the reason says which.
    Dropped { reason: String },
}

/// A failure the loop found, with its scenario and the ruling on it.
pub struct Found {
    pub scenario: Scenario,
    pub failure: Failure,
    pub ruling: Ruling,
}

/// One scenario's outcome: its origin, whether it passed, and its timings.
pub struct Outcome {
    pub origin: Origin,
    pub passed: bool,
    pub timings: Vec<Timed>,
}

/// What an execution of the step decided. Its evidence is numbered from 1 across the scenarios.
pub struct Execution {
    pub target: String,
    pub outcomes: Vec<Outcome>,
    pub found: Vec<Found>,
    pub evidence: Vec<crate::recipes::runs::EvidenceRow>,
}

impl Execution {
    /// The findings the verifier confirmed: the step fails when there are any.
    pub fn confirmed(&self) -> usize {
        self.found
            .iter()
            .filter(|f| matches!(f.ruling, Ruling::Confirmed { .. }))
            .count()
    }

    /// The step's summary in a line: what ran, and what the verifier ruled.
    pub fn summary(&self) -> String {
        let of = |origin: Origin| self.outcomes.iter().filter(|o| o.origin == origin).count();
        let dropped = self
            .found
            .iter()
            .filter(|f| matches!(f.ruling, Ruling::Dropped { .. }))
            .count();
        let confirmed: Vec<String> = self
            .found
            .iter()
            .filter(|f| matches!(f.ruling, Ruling::Confirmed { .. }))
            .map(|f| format!("{}: {}", f.scenario.title, f.failure.text()))
            .collect();
        let verdicts = if confirmed.is_empty() {
            "no finding confirmed".to_string()
        } else {
            format!("confirmed: {}", confirmed.join("; "))
        };
        let passed = self.outcomes.iter().filter(|o| o.passed).count();
        let timed: Vec<String> = self
            .outcomes
            .iter()
            .flat_map(|o| o.timings.iter())
            .filter_map(|t| {
                t.ms.map(|ms| format!("{} {ms} ms of {} ms", t.name, t.budget_ms))
            })
            .collect();
        let timings = if timed.is_empty() {
            String::new()
        } else {
            format!(", timings: {}", timed.join("; "))
        };
        format!(
            "{} scenarios ({} acceptance, {} regression, {} exploratory, {} accessibility), {passed} passed{timings}; {dropped} dropped on the verifier's re-run or by the planner; {verdicts}",
            self.outcomes.len(),
            of(Origin::Acceptance),
            of(Origin::Regression),
            of(Origin::Exploratory),
            of(Origin::Accessibility),
        )
    }
}

/// Runs the step: the planner's scenarios, the regressions and the accessibility scenario, each on a fresh target from
/// `open`; the failures are judged, re-run and ruled on. An error is the step failing to run (the models or the target),
/// not a finding.
pub fn execute<'a>(
    ctx: &Ctx,
    input: &Input,
    open: &mut dyn FnMut() -> Result<Box<dyn Target + 'a>, String>,
) -> Result<Execution, String> {
    let titles: Vec<String> = input.regressions.iter().map(|s| s.title.clone()).collect();
    let planned = ctx.models.plan(&PlanRequest {
        symbol: input.symbol,
        acceptance: input.acceptance,
        regressions: &titles,
        exploratory: input.exploratory,
    })?;
    let mut scenarios = Vec::new();
    let mut explored = 0;
    for mut s in planned {
        if s.origin == Origin::Exploratory {
            if explored >= input.exploratory {
                continue;
            }
            explored += 1;
        } else {
            s.origin = Origin::Acceptance;
        }
        scenarios.push(s);
    }
    for regression in input.regressions {
        let mut s = regression.clone();
        s.origin = Origin::Regression;
        scenarios.push(s);
    }
    scenarios.push(accessibility(input.symbol));

    let mut target_kind = String::new();
    let mut outcomes = Vec::new();
    let mut found = Vec::new();
    let mut evidence = Vec::new();
    for scenario in &scenarios {
        let mut target = open()?;
        if target_kind.is_empty() {
            target_kind = target.kind().to_string();
        }
        let first = run_scenario(ctx, target.as_mut(), scenario)?;
        evidence.extend(first.evidence);
        outcomes.push(Outcome {
            origin: scenario.origin,
            passed: first.failure.is_none(),
            timings: first.timings,
        });
        let Some(failure) = first.failure else {
            continue;
        };
        let judged = ctx.models.judge(scenario, &failure.text())?;
        if !judged.defect {
            found.push(Found {
                scenario: scenario.clone(),
                failure,
                ruling: Ruling::Dropped {
                    reason: format!(
                        "the planner read it as not a defect of the product: {}",
                        judged.reason
                    ),
                },
            });
            continue;
        }
        // The verifier's re-run: a fresh target, the same scenario.
        let mut again = open()?;
        let second = run_scenario(ctx, again.as_mut(), scenario)?;
        let rerun_evidence = second
            .evidence
            .last()
            .map(|r| r.tree.clone())
            .unwrap_or_default();
        evidence.extend(second.evidence);
        let ruling = match second.failure {
            Some(repeat) if repeat.same_kind(&failure) => Ruling::Confirmed {
                proof: format!(
                    "the re-run failed again: {}; evidence {rerun_evidence}",
                    repeat.text()
                ),
            },
            _ => Ruling::Dropped {
                reason: format!(
                    "did not fail again on a re-run, so it is not a finding: {}",
                    failure.text()
                ),
            },
        };
        found.push(Found {
            scenario: scenario.clone(),
            failure,
            ruling,
        });
    }
    for (n, row) in evidence.iter_mut().enumerate() {
        row.seq = n as u32 + 1;
    }
    Ok(Execution {
        target: target_kind,
        outcomes,
        found,
        evidence,
    })
}

/// A finding for the review run: its axis (`fea` for a scenario's failure, `prf` for a timing), and the finding. Its
/// scenario is the scenario's JSON, which the next run reads as a regression.
fn finding_of(found: &Found, roles: &Roles, symbol: &str) -> (&'static str, Finding) {
    let performance = matches!(found.failure, Failure::Performance { .. });
    let (verdict, proof, reason) = match &found.ruling {
        Ruling::Confirmed { proof } => (Verdict::Confirmed, proof.clone(), String::new()),
        Ruling::Dropped { reason } => (Verdict::Dropped, String::new(), reason.clone()),
    };
    let scenario = &found.scenario;
    let finding = Finding {
        symbol: if scenario.symbol.is_empty() {
            symbol.to_string()
        } else {
            scenario.symbol.clone()
        },
        lineage: None,
        text: format!("{}: {}", scenario.title, found.failure.text()),
        severity: if scenario.origin == Origin::Exploratory {
            "minor"
        } else {
            "major"
        }
        .into(),
        node: String::new(),
        round: 1,
        id: String::new(),
        scenario: serde_json::to_string(scenario).unwrap_or_default(),
        kind: if performance {
            "performance"
        } else {
            "user-test"
        }
        .into(),
        verdict,
        proof,
        reason,
        by: Some(by("user", &roles.driver)),
        verified_by: Some(by("verifier", &roles.driver)),
        contest: String::new(),
        ruling: String::new(),
        ruling_reason: String::new(),
    };
    (if performance { "prf" } else { "fea" }, finding)
}

/// The runs whose user-test steps wait for the scheduler (the fire route queues them).
static QUEUED: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn queued() -> MutexGuard<'static, Vec<String>> {
    QUEUED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Asks for the user-test steps of recipe run `run` to be run.
pub fn queue(run: &str) {
    let mut waiting = queued();
    if !waiting.iter().any(|r| r == run) {
        waiting.push(run.to_string());
    }
}

/// The scheduler's entry point (every pass): each queued run starts on its own thread.
pub fn tick(app: &Arc<App>) {
    let runs = std::mem::take(&mut *queued());
    for run in runs {
        let app = Arc::clone(app);
        std::thread::spawn(move || {
            if let Err(why) = run_queued(&app, &run) {
                eprintln!("apid: user tests of run {run}: {why}");
            }
        });
    }
}

/// Runs the first user-test step of a queued run that is waiting (started or ready), and asks again for the run when the
/// step's settling makes another one ready.
fn run_queued(app: &App, run: &str) -> Result<(), String> {
    let Some(view) = app.recipe_runs.view(run) else {
        return Ok(());
    };
    let waiting = view.recipe.steps.iter().find(|s| {
        s.engine.as_deref() == Some(USER_TEST)
            && view
                .walk
                .steps
                .get(&s.id)
                .is_some_and(|r| matches!(r.state, StepState::Running | StepState::Ready))
    });
    let Some(step) = waiting else {
        return Ok(());
    };
    let now = apz_plaza::now_ms();
    if view.walk.steps.get(&step.id).map(|r| r.state) == Some(StepState::Ready) {
        app.recipe_runs.started(run, &step.id, now);
    }
    let project = view.run.project.clone().unwrap_or_default();
    let change = view.run.subject.clone().unwrap_or_default();
    let moves = match &step.user {
        Some(config) => run_step(app, run, &step.id, config, &project, &change, now)?,
        None => finish(
            app,
            run,
            &step.id,
            Settled {
                passed: false,
                findings: 0,
                commands: "no config".into(),
                summary: "the step has no [user] table".into(),
            },
            now,
        ),
    };
    if moves.iter().any(|m| matches!(m, Move::Ready(_))) {
        queue(run);
    }
    Ok(())
}

/// How a step ended: its verdict, the findings it confirmed, the commands it ran (the target's kind) and a summary.
struct Settled {
    passed: bool,
    findings: u32,
    commands: String,
    summary: String,
}

/// Settles a step: its check row on the run, and the walk's move on its report.
fn finish(app: &App, run: &str, step: &str, settled: Settled, now: u64) -> Vec<Move> {
    app.recipe_runs.record_check(
        run,
        CheckRow {
            step: step.to_string(),
            passed: settled.passed,
            commands: settled.commands,
            summary: settled.summary,
        },
        now,
    );
    app.recipe_runs.settle(
        run,
        step,
        &Report {
            passed: settled.passed,
            findings: Some(settled.findings),
            ..Report::default()
        },
        now,
    )
}

/// Opens a fresh target for each scenario and each re-run.
type Opener<'a> = Box<dyn FnMut() -> Result<Box<dyn Target + 'a>, String> + 'a>;

/// The lane's test fork of a fork-placed target (PLAN 19.3): the fork, the node and the VM it runs in, and the port of the
/// app a web target's page is served from. The hub retires the fork when the step's scenarios are done.
pub struct Placed<'h> {
    pub fork: RefCell<Fork<'h, Live<'h>>>,
    pub node: String,
    pub run: String,
    pub cid: u64,
    pub app_port: u16,
}

/// The fork a step's web or terminal target runs in, or `None` for the stand-in. A web target's fork is the lane's
/// preview base at the lane's revision, serving the app (the preview target's `serve`) and running the browser in it; a
/// terminal target's fork is the test base at the lane's revision.
fn placed_for<'h>(
    app: &App,
    host: &'h Live<'h>,
    config: &UserTest,
    project: &str,
    change: &str,
) -> Result<Option<Placed<'h>>, String> {
    let fork_gui = config.desktop.as_ref().is_some_and(|desk| desk.os == LINUX);
    if config.stand_in.is_some()
        || config.macos.is_some()
        || config.simulator.is_some()
        || config.android.is_some()
        || (config.desktop.is_some() && !fork_gui)
    {
        return Ok(None);
    }
    if config.web.is_none() && config.tui.is_none() && !fork_gui {
        return Err(
            "the step names no target (stand_in, web, tui, desktop, macos or simulator)".into(),
        );
    }
    let rev = project_memory::landed_revision(app, change)?;
    let stack = project_stack::read(app, project)?
        .ok_or_else(|| format!("{} is not on main", project_stack::FILE))?;
    let repo = app
        .projects
        .get(project)
        .and_then(|p| p.repo)
        .ok_or_else(|| format!("{project} has no repo yet"))?;
    let pool = base_builds::pool_keys(&stack)
        .first()
        .cloned()
        .unwrap_or_default();
    let node = placement::base_node(app, project, &pool, &base_recipe::volumes(&stack))
        .ok_or_else(|| format!("no node in the pool {pool} has room for a fork"))?;
    let (steps, app_port) = if config.web.is_some() {
        let target = stack
            .targets
            .iter()
            .find(|t| t.kind == Some(TargetKind::Preview))
            .ok_or("the project has no preview target to serve the page's app")?;
        let (serve, port) = match (
            target
                .serve
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
            target.port.filter(|port| *port != 0),
        ) {
            (Some(serve), Some(port)) => (serve, port),
            _ => {
                return Err(format!(
                    "preview target {} needs a serve command and a port",
                    target.name
                ));
            }
        };
        (
            base_recipe::preview_steps(&stack, &repo, &rev, serve, port),
            port,
        )
    } else {
        (base_recipe::fork_steps(&stack, &repo, &rev), 0)
    };
    let fork = Fork::start(host, node.clone(), project.to_string(), &steps)?;
    let Some((run, cid)) = fork.location() else {
        let _ = fork.retire();
        return Err("the test fork has no VM".into());
    };
    let placed = Placed {
        fork: RefCell::new(fork),
        node,
        run,
        cid,
        app_port,
    };
    if config.web.is_some()
        && let Err(why) = start_browser(host, &placed)
    {
        retire_placed(placed);
        return Err(why);
    }
    if fork_gui && let Err(why) = start_agent(host, &placed) {
        retire_placed(placed);
        return Err(why);
    }
    Ok(Some(placed))
}

/// Starts the browser in a web target's fork and makes its DevTools port reachable (6.4: the guest's announcement, or
/// noded's registration when none comes).
fn start_browser(host: &Live<'_>, placed: &Placed<'_>) -> Result<(), String> {
    let answer = placed.fork.borrow_mut().background(&browser_command())?;
    if !answer.ok {
        return Err(format!(
            "the browser did not start in the fork: {}",
            answer.output.trim_end()
        ));
    }
    publish_port(host, &placed.node, &placed.run, placed.cid, DEVTOOLS_PORT)
}

/// Starts the desktop agent in a Linux desktop target's fork and makes its port reachable, as the browser's is.
fn start_agent(host: &Live<'_>, placed: &Placed<'_>) -> Result<(), String> {
    let answer = placed.fork.borrow_mut().background(&agent_command())?;
    if !answer.ok {
        return Err(format!(
            "the desktop agent did not start in the fork: {}",
            answer.output.trim_end()
        ));
    }
    publish_port(host, &placed.node, &placed.run, placed.cid, AGENT_PORT)
}

/// Retires a placed fork: its VM is stopped.
fn retire_placed(placed: Placed<'_>) {
    if let Err(why) = placed.fork.into_inner().retire() {
        eprintln!("apid: a user test's fork: {why}");
    }
}

/// A test fork of the project's base at `commit` with its browser started: the fork a web target's page is opened in when no
/// app of the change is served (a deploy's smoke set, 23). Its node is one the pool can place the base on.
fn browser_fork<'h>(
    app: &App,
    host: &'h Live<'h>,
    project: &str,
    commit: &str,
) -> Result<Placed<'h>, String> {
    let stack = project_stack::read(app, project)?
        .ok_or_else(|| format!("{} is not on main", project_stack::FILE))?;
    let repo = app
        .projects
        .get(project)
        .and_then(|p| p.repo)
        .ok_or_else(|| format!("{project} has no repo yet"))?;
    let pool = base_builds::pool_keys(&stack)
        .first()
        .cloned()
        .unwrap_or_default();
    let node = placement::base_node(app, project, &pool, &base_recipe::volumes(&stack))
        .ok_or_else(|| format!("no node in the pool {pool} has room for a fork"))?;
    let steps = base_recipe::fork_steps(&stack, &repo, commit);
    let fork = Fork::start(host, node.clone(), project.to_string(), &steps)?;
    let Some((run, cid)) = fork.location() else {
        let _ = fork.retire();
        return Err("the test fork has no VM".into());
    };
    let placed = Placed {
        fork: RefCell::new(fork),
        node,
        run,
        cid,
        app_port: 0,
    };
    if let Err(why) = start_browser(host, &placed) {
        retire_placed(placed);
        return Err(why);
    }
    Ok(placed)
}

/// The smoke set of a deploy (23, 8.2): its scenarios run through a web target at `url`, in a browser of the test fork of the
/// release's `commit`. The first scenario that does not hold is the error. The fork is retired when the set is done.
pub fn smoke(
    app: &App,
    project: &str,
    url: &str,
    commit: &str,
    scenarios: &[Scenario],
) -> Result<(), String> {
    let host = Live(app);
    let placed = browser_fork(app, &host, project, commit)?;
    let result = {
        let mut dial = port_dial(
            &app.nodes,
            &placed.node,
            placed.cid,
            DEVTOOLS_PORT,
            LOCAL_INBOUND,
        );
        run_smoke(
            app.blobs.as_ref(),
            &mut || Ok::<Box<dyn Target>, String>(Box::new(Web::attach(&mut dial, url)?)),
            scenarios,
        )
    };
    retire_placed(placed);
    result
}

/// Runs a smoke set on the targets `open` opens, a fresh one for each scenario. No driver and no checker is asked: a smoke
/// scenario's expectations are read from the screen as it is, and one that does not hold fails (a step with actions, which
/// a smoke set does not take, fails as the models refuse). The first failure is the error.
pub fn run_smoke<'a>(
    blobs: &Blobs,
    open: &mut dyn FnMut() -> Result<Box<dyn Target + 'a>, String>,
    scenarios: &[Scenario],
) -> Result<(), String> {
    let ctx = Ctx {
        models: &NoDriver,
        blobs,
        threshold: 1.0,
        actions: 0,
    };
    for scenario in scenarios {
        let mut target = open()?;
        let run = run_scenario(&ctx, target.as_mut(), scenario)?;
        if let Some(failure) = run.failure {
            return Err(format!("{}: {}", scenario.title, failure.text()));
        }
    }
    Ok(())
}

/// The models of a smoke run, which asks none of them to act: a smoke set takes no action, and its checker reads a screen
/// that has a tree (a screen with none fails its expectation).
struct NoDriver;

impl Models for NoDriver {
    fn plan(&self, _req: &PlanRequest) -> Result<Vec<Scenario>, String> {
        Err("a smoke set is written before its run, not planned by it".into())
    }

    fn judge(&self, _scenario: &Scenario, _failure: &str) -> Result<Judgement, String> {
        Ok(Judgement {
            defect: true,
            reason: "a smoke scenario that fails is a defect of the deploy".into(),
        })
    }

    fn pick(
        &self,
        _tier: Tier,
        _req: &PickRequest,
        _shot: Option<&Screenshot>,
    ) -> Result<Pick, String> {
        Err("a smoke set takes no action".into())
    }

    fn look(&self, _question: &str, _shot: &Screenshot) -> Result<Look, String> {
        Ok(Look {
            holds: false,
            reason: "a smoke set reads the screen's tree, and this screen has none".into(),
        })
    }
}

/// The opener of the step's target: the stand-in (scripted data, in-process), a page of the fork's browser, or a program in
/// the fork's pseudo-terminal.
fn opener<'a, 'h>(
    app: &'a App,
    config: &'a UserTest,
    placed: Option<&'a Placed<'h>>,
    actor: Option<u64>,
) -> Result<Opener<'a>, String> {
    if let Some(stand) = &config.stand_in {
        let stand = stand.clone();
        let open: Opener<'a> = Box::new(move || -> Result<Box<dyn Target + 'a>, String> {
            Ok(Box::new(stand.open()))
        });
        return Ok(open);
    }
    if let Some(desk) = &config.desktop {
        if desk.os == LINUX {
            let placed = placed.ok_or("the Linux desktop's fork is not placed")?;
            let open: Opener<'a> = Box::new(move || -> Result<Box<dyn Target + 'a>, String> {
                let dial = port_dial(
                    &app.nodes,
                    &placed.node,
                    placed.cid,
                    AGENT_PORT,
                    LOCAL_INBOUND,
                );
                Ok(Box::new(Gui::open(
                    Box::new(Forked::new(dial)),
                    FORK_STATION,
                    LINUX,
                )?))
            });
            return Ok(open);
        }
        let station = desk
            .station
            .clone()
            .ok_or("a Windows desktop names its station")?;
        let cid = actor.ok_or(
            "the run has no agent on the station: a test step acts as its own session, and none is attached to this run",
        )?;
        let os = desk.os.clone();
        let open: Opener<'a> = Box::new(move || -> Result<Box<dyn Target + 'a>, String> {
            Ok(Box::new(Gui::open(
                Box::new(Hub { app, cid }),
                &station,
                &os,
            )?))
        });
        return Ok(open);
    }
    if let Some(android) = &config.android {
        return Ok(android::opener(app, android));
    }
    if config.macos.is_some() || config.simulator.is_some() {
        return desk::opener(app, config);
    }
    let placed = placed
        .ok_or("the step names no target (stand_in, web, tui, desktop, macos or simulator)")?;
    if let Some(page) = &config.web {
        let url = format!(
            "http://127.0.0.1:{}/{}",
            placed.app_port,
            page.path.trim_start_matches('/')
        );
        let mut dial = port_dial(
            &app.nodes,
            &placed.node,
            placed.cid,
            DEVTOOLS_PORT,
            LOCAL_INBOUND,
        );
        let open: Opener<'a> = Box::new(move || -> Result<Box<dyn Target + 'a>, String> {
            Ok(Box::new(Web::attach(&mut dial, &url)?))
        });
        return Ok(open);
    }
    let Some(tui) = &config.tui else {
        return Err(
            "the step names no target (stand_in, web, tui, desktop, macos or simulator)".into(),
        );
    };
    let channel: &'a dyn Channel = &placed.fork;
    let (command, cols, rows) = (tui.command.clone(), tui.cols, tui.rows);
    let open: Opener<'a> = Box::new(move || -> Result<Box<dyn Target + 'a>, String> {
        Ok(Box::new(ForkTui::open(channel, &command, cols, rows)?))
    });
    Ok(open)
}

/// What a step's scenarios came to: the execution, or why no target or no run.
enum Ran {
    NoTarget(String),
    Failed(String),
    Done(Execution),
}

/// The agent a step's station calls act as: the one attached to the run, so the laws and the audit name the session that
/// runs the step. `None` when no agent is attached; the calls then refuse rather than act as anyone else.
fn run_session(app: &App, run: &str) -> Option<u64> {
    app.agents
        .select(|agent| (agent.run.as_deref() == Some(run)).then_some(agent.cid))
        .into_iter()
        .min()
}

/// Runs the step's scenarios on its target, with the gateway's models and the project's regressions.
fn run_scenarios(
    app: &App,
    config: &UserTest,
    project: &str,
    change: &str,
    roles: &Roles,
    placed: Option<&Placed<'_>>,
    actor: Option<u64>,
) -> Ran {
    let mut open = match opener(app, config, placed, actor) {
        Ok(open) => open,
        Err(why) => return Ran::NoTarget(why),
    };
    let gateway = app.providers.gateway();
    let models = GatewayModels {
        gateway: &gateway,
        roles,
    };
    let mut regressions: Vec<Scenario> = Vec::new();
    for json in app.reviews.confirmed_scenarios(project) {
        if let Ok(s) = serde_json::from_str::<Scenario>(&json)
            && s.check().is_ok()
            && !regressions.iter().any(|r| r.id == s.id)
        {
            regressions.push(s);
        }
    }
    let ctx = Ctx {
        models: &models as &dyn Models,
        blobs: app.blobs.as_ref(),
        threshold: config.threshold,
        actions: config.actions,
    };
    let input = Input {
        symbol: change,
        acceptance: &config.acceptance,
        regressions: &regressions,
        exploratory: config.exploratory,
    };
    match execute(&ctx, &input, &mut *open) {
        Ok(execution) => Ran::Done(execution),
        Err(why) => Ran::Failed(why),
    }
}

/// A step's summary with a note the desktop target's end added (the recording's artifact, a failure to quit).
fn with_note(summary: String, note: &str) -> String {
    if note.is_empty() {
        summary
    } else {
        format!("{summary}; {note}")
    }
}

/// A device-farm step (PLAN 19.3, 8.5): the farm's suite runs on its devices through the farm's connector, and the results come
/// back as the run's evidence. The step has no scenarios, so it has no findings: a failing session fails the step.
fn run_farm(app: &App, run: &str, step: &str, config: &FarmTest, now: u64) -> Vec<Move> {
    let settle = |passed: bool, summary: String| {
        finish(
            app,
            run,
            step,
            Settled {
                passed,
                findings: 0,
                commands: format!("{} device farm", config.farm),
                summary,
            },
            now,
        )
    };
    let (Some((_, app_bytes)), Some((_, test_bytes))) = (
        app.artifacts.blob(&config.app),
        app.artifacts.blob(&config.tests),
    ) else {
        return settle(
            false,
            "the app or the tests are not blobs of the hub, so the farm run did not start".into(),
        );
    };
    let deps = farm::Deps {
        outbound: &crate::glue::HttpsOutbound,
        vault: app.vault.as_ref(),
        pace: farm::Pace::PRODUCTION,
    };
    match farm::run(&deps, config, &app_bytes, &test_bytes) {
        Ok(report) => {
            let sessions = report.sessions.len();
            let passed = report.sessions.iter().filter(|s| s.passed).count();
            app.recipe_runs
                .record_evidence(run, farm::evidence(&report, &config.farm), now);
            settle(
                report.passed(),
                format!(
                    "{} run {}: {passed} of {sessions} sessions passed; the results are the run's evidence",
                    config.farm, report.run
                ),
            )
        }
        Err(why) => settle(
            false,
            format!("the {} run did not finish: {why}", config.farm),
        ),
    }
}

/// One `test:user` step of a run, from its config: the models, the target and the regressions of the project.
fn run_step(
    app: &App,
    run: &str,
    step: &str,
    config: &UserTest,
    project: &str,
    change: &str,
    now: u64,
) -> Result<Vec<Move>, String> {
    if let Some(farm_config) = &config.farm {
        return Ok(run_farm(app, run, step, farm_config, now));
    }
    let roles = Roles {
        planner: config.planner.clone(),
        driver: config.driver.clone(),
        escalate: config.escalate.clone(),
        checker: config.checker.clone(),
    };
    let no_target = |why: String| {
        finish(
            app,
            run,
            step,
            Settled {
                passed: false,
                findings: 0,
                commands: "no target".into(),
                summary: format!("no target: {why}, so no scenario ran"),
            },
            now,
        )
    };
    let setup = match desk::prepare(app, config) {
        Ok(setup) => setup,
        Err(why) => return Ok(no_target(why)),
    };
    let host = Live(app);
    let placed = match placed_for(app, &host, config, project, change) {
        Ok(placed) => placed,
        Err(why) => {
            let note = desk::finish(app, config, setup);
            return Ok(no_target(with_note(why, &note)));
        }
    };
    let actor = run_session(app, run);
    let ran = run_scenarios(app, config, project, change, &roles, placed.as_ref(), actor);
    // The desktop target's end: a Mac app is quit, the simulator's video lands, the simulator shuts down.
    let note = desk::finish(app, config, setup);
    if let Some(placed) = placed {
        retire_placed(placed);
    }
    let execution = match ran {
        Ran::NoTarget(why) => return Ok(no_target(with_note(why, &note))),
        Ran::Failed(why) => {
            return Ok(finish(
                app,
                run,
                step,
                Settled {
                    passed: false,
                    findings: 0,
                    commands: "the user test".into(),
                    summary: with_note(format!("the user test did not run: {why}"), &note),
                },
                now,
            ));
        }
        Ran::Done(execution) => execution,
    };
    let found: Vec<(&'static str, Finding)> = execution
        .found
        .iter()
        .map(|f| finding_of(f, &roles, change))
        .collect();
    let review = format!("ut-{run}");
    app.reviews
        .record_user_run(&review, change, project, change, found, now)?;
    let confirmed = execution.confirmed();
    let commands = format!("{} target", execution.target);
    let summary = with_note(execution.summary(), &note);
    app.recipe_runs
        .record_evidence(run, execution.evidence, now);
    Ok(finish(
        app,
        run,
        step,
        Settled {
            passed: confirmed == 0,
            findings: confirmed as u32,
            commands,
            summary,
        },
        now,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user_test::target::{Element, Link, Screen, StandIn};
    use apz_blobs::Blobs;
    use apz_gateway::{Gateway, ProviderConfig, ProviderKind};
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use std::sync::OnceLock;

    /// The gateway's scripted models: every test's answers are in one script, keyed by the markers of its prompts.
    // The test sets the process variable the mock provider reads, once, before any test reads it (as providers.rs does).
    #[allow(unsafe_code)]
    fn script() -> &'static PathBuf {
        static PATH: OnceLock<PathBuf> = OnceLock::new();
        PATH.get_or_init(|| {
            let path = std::env::temp_dir().join(format!("apid-user-test-script-{}.json", std::process::id()));
            let entries = json!([
                { "when": ["ROLE: planner plan", "SYMBOL: cart-ok"], "turns": [{ "text": plan("press Pay", "Receipt", None) }] },
                { "when": ["ROLE: planner plan", "SYMBOL: cart-bug"], "turns": [{ "text": plan("press Pay", "Receipt", None) }] },
                { "when": ["ROLE: planner plan", "SYMBOL: cart-flaky"], "turns": [{ "text": plan("press Pay", "Receipt", None) }] },
                { "when": ["ROLE: planner plan", "SYMBOL: cart-slow"], "turns": [{ "text": plan("press Pay", "Receipt", Some(500)) }] },
                { "when": ["ROLE: planner plan", "SYMBOL: cart-escalate"], "turns": [{ "text": plan("settle the bill", "Receipt", None) }] },
                { "when": ["ROLE: planner judge"], "turns": [{ "text": "{\"verdict\": \"defect\", \"reason\": \"the receipt is not shown\"}" }] },
                { "when": ["ROLE: driver tier fast", "GOAL: press Pay"], "turns": [{ "text": "{\"action\": \"click\", \"element\": \"pay\", \"confidence\": 0.9, \"why\": \"Pay is the button\"}" }] },
                { "when": ["ROLE: driver tier fast", "GOAL: settle"], "turns": [{ "text": "{\"action\": \"click\", \"element\": \"pay\", \"confidence\": 0.2, \"why\": \"unsure\"}" }] },
                { "when": ["ROLE: driver tier strong", "GOAL: settle", "IMAGE: image/png"], "turns": [{ "text": "{\"action\": \"click\", \"element\": \"pay\", \"confidence\": 0.95, \"why\": \"Pay settles it, from the picture\"}" }] },
                { "when": ["ROLE: driver tier strong", "GOAL: settle"], "turns": [{ "text": "{\"action\": \"click\", \"element\": \"pay\", \"confidence\": 0.95, \"why\": \"Pay settles it\"}" }] },
                { "when": ["ROLE: checker", "IMAGE: image/png"], "turns": [{ "text": "{\"holds\": true, \"reason\": \"the screenshot arrived\"}" }] },
                { "when": ["ROLE: checker"], "turns": [{ "text": "{\"holds\": true, \"reason\": \"the screen shows it\"}" }] }
            ]);
            std::fs::write(&path, entries.to_string()).expect("the test script is written");
            unsafe { std::env::set_var("APZ_MOCK_SCRIPT", &path) };
            path
        })
    }

    /// A planner's answer: one acceptance scenario whose step is `goal` and expects `shows`, with an exploratory one.
    fn plan(goal: &str, shows: &str, budget_ms: Option<u64>) -> String {
        let mut pay = json!({
            "id": "pay", "title": "Paying reaches the receipt", "origin": "acceptance", "symbol": "cart::pay",
            "steps": [{ "goal": goal, "expect": [{ "present": shows }] }]
        });
        if let Some(budget_ms) = budget_ms {
            pay["timings"] = json!([{ "name": "pay", "from": 0, "to": 0, "budget_ms": budget_ms }]);
        }
        json!({ "scenarios": [pay, {
            "id": "look", "title": "Look around", "origin": "exploratory",
            "steps": [{ "goal": "look around", "expect": [] }]
        }] })
        .to_string()
    }

    fn el(id: &str, role: &str, label: &str) -> Element {
        Element {
            id: id.into(),
            role: role.into(),
            label: label.into(),
            value: String::new(),
        }
    }

    /// The shop: the cart's Pay goes to the receipt (or, seeded, to an error screen).
    fn shop(to: &str, latency_ms: u64) -> StandIn {
        StandIn {
            start: "cart".into(),
            latency_ms,
            screens: vec![
                Screen {
                    id: "cart".into(),
                    title: "Cart".into(),
                    elements: vec![el("pay", "button", "Pay"), el("note", "textbox", "Note")],
                    on: vec![Link {
                        element: "pay".into(),
                        to: to.into(),
                    }],
                },
                Screen {
                    id: "receipt".into(),
                    title: "Receipt".into(),
                    elements: vec![el("home", "link", "Back to shop")],
                    on: vec![],
                },
                Screen {
                    id: "error".into(),
                    title: "Error".into(),
                    elements: vec![el("retry", "button", "Retry")],
                    on: vec![],
                },
            ],
        }
    }

    /// The gateway with its `mock` provider, whose `script` and `script-review` models answer from the script.
    fn gateway() -> Gateway {
        script();
        Gateway::builder()
            .providers(vec![ProviderConfig::new("mock", ProviderKind::Mock)])
            .build()
    }

    fn roles(escalate: &str) -> Roles {
        Roles {
            planner: "mock/script".into(),
            driver: "mock/script".into(),
            escalate: escalate.into(),
            checker: "mock/script".into(),
        }
    }

    /// Runs the scenarios of the stand-in `stand` (opened by `reopen`) for the change `symbol`.
    fn run_with(
        symbol: &str,
        stand: StandIn,
        escalate: &str,
        threshold: f64,
        reopen: &mut dyn FnMut(&StandIn) -> Box<dyn Target>,
    ) -> Execution {
        let dir = tempfile::tempdir().unwrap();
        let blobs = Blobs::open(dir.path()).unwrap();
        let roles = roles(escalate);
        let gateway = gateway();
        let models = GatewayModels {
            gateway: &gateway,
            roles: &roles,
        };
        let ctx = Ctx {
            models: &models,
            blobs: &blobs,
            threshold,
            actions: 4,
        };
        let input = Input {
            symbol,
            acceptance: &[],
            regressions: &[],
            exploratory: 1,
        };
        let mut open = || -> Result<Box<dyn Target>, String> { Ok(reopen(&stand)) };
        execute(&ctx, &input, &mut open).expect("the user test runs")
    }

    #[test]
    fn a_scenario_runs_on_the_stand_in_and_records_its_evidence() {
        let exec = run_with(
            "cart-ok",
            shop("receipt", 0),
            "mock/script",
            0.6,
            &mut |s| Box::new(s.open()),
        );
        assert_eq!(exec.target, "stand-in");
        assert!(exec.found.is_empty(), "the acceptance scenario passes");
        let origins: Vec<Origin> = exec.outcomes.iter().map(|o| o.origin).collect();
        assert_eq!(
            origins,
            [
                Origin::Acceptance,
                Origin::Exploratory,
                Origin::Accessibility
            ]
        );
        // The press (the tree changes), the holding look, the exploratory look, the accessibility read.
        let first = &exec.evidence[0];
        assert_eq!(first.action, "click pay");
        assert!(!first.tree.is_empty() && !first.screenshot.is_empty());
        assert!(exec.evidence.iter().any(|r| r.note == "holds"));
        assert_eq!(
            exec.evidence.iter().map(|r| r.seq).collect::<Vec<_>>(),
            (1..=exec.evidence.len() as u32).collect::<Vec<_>>()
        );
        assert_eq!(exec.confirmed(), 0);
    }

    #[test]
    fn the_driver_escalates_below_its_threshold_and_the_stronger_model_picks() {
        let exec = run_with(
            "cart-escalate",
            shop("receipt", 0),
            "mock/script",
            0.6,
            &mut |s| Box::new(s.open()),
        );
        // The stronger model's answer names the picture: the screenshot went to it as an image.
        let escalated = exec
            .evidence
            .iter()
            .find(|r| r.action == "click pay")
            .expect("the strong model's pick is taken");
        assert!(
            escalated
                .note
                .starts_with("escalated: Pay settles it, from the picture"),
            "{}",
            escalated.note
        );
        assert!(exec.found.is_empty());
    }

    #[test]
    fn the_checker_is_sent_the_screenshot_as_an_image() {
        let gw = gateway();
        let roles = roles("mock/script");
        let models = GatewayModels {
            gateway: &gw,
            roles: &roles,
        };
        let mut stand = shop("receipt", 0).open();
        let shot = stand.screenshot().unwrap();
        assert_eq!(shot.mime, "image/png");
        // The script answers for the picture only when an image block arrived (the mock names it in the text it reads).
        let look = models
            .look("Does the screen show the receipt?", &shot)
            .unwrap();
        assert!(look.holds);
        assert_eq!(look.reason, "the screenshot arrived");
    }

    #[test]
    fn a_failing_scenario_is_a_finding_that_the_rerun_confirms() {
        let exec = run_with("cart-bug", shop("error", 0), "mock/script", 0.6, &mut |s| {
            Box::new(s.open())
        });
        assert_eq!(exec.found.len(), 1);
        let found = &exec.found[0];
        assert!(matches!(
            found.failure,
            Failure::Feature { step: Some(0), .. }
        ));
        assert!(
            matches!(found.ruling, Ruling::Confirmed { .. }),
            "{:?}",
            found.ruling
        );
        assert_eq!(exec.confirmed(), 1);
        let (axis, finding) = finding_of(found, &roles("mock/script"), "cart-bug");
        assert_eq!(axis, "fea");
        assert_eq!(finding.kind, "user-test");
        assert_eq!(finding.verdict, Verdict::Confirmed);
        assert!(serde_json::from_str::<Value>(&finding.scenario).is_ok());
    }

    #[test]
    fn a_failure_that_does_not_repeat_on_the_rerun_is_dropped_as_flaky() {
        // The first target is seeded wrong, the re-run's target is right: the failure does not hold again.
        let mut first = true;
        let exec = run_with(
            "cart-flaky",
            shop("receipt", 0),
            "mock/script",
            0.6,
            &mut |s| {
                let target = if first {
                    shop("error", 0).open()
                } else {
                    s.open()
                };
                first = false;
                Box::new(target)
            },
        );
        assert_eq!(exec.found.len(), 1);
        assert!(matches!(exec.found[0].ruling, Ruling::Dropped { .. }));
        assert_eq!(exec.confirmed(), 0);
    }

    #[test]
    fn a_timing_over_its_budget_is_a_performance_finding_on_the_prf_axis() {
        let exec = run_with(
            "cart-slow",
            shop("receipt", 800),
            "mock/script",
            0.6,
            &mut |s| Box::new(s.open()),
        );
        assert_eq!(exec.found.len(), 1);
        let found = &exec.found[0];
        assert!(matches!(
            found.failure,
            Failure::Performance {
                ms: 800,
                budget_ms: 500,
                ..
            }
        ));
        assert!(matches!(found.ruling, Ruling::Confirmed { .. }));
        let (axis, finding) = finding_of(found, &roles("mock/script"), "cart-slow");
        assert_eq!((axis, finding.kind.as_str()), ("prf", "performance"));
    }

    #[test]
    fn the_summary_says_what_ran_and_what_was_confirmed() {
        let exec = run_with("cart-bug", shop("error", 0), "mock/script", 0.6, &mut |s| {
            Box::new(s.open())
        });
        let summary = exec.summary();
        assert!(
            summary.starts_with(
                "3 scenarios (1 acceptance, 0 regression, 1 exploratory, 1 accessibility)"
            ),
            "{summary}"
        );
        assert!(
            summary.contains("confirmed: Paying reaches the receipt"),
            "{summary}"
        );
    }
}
