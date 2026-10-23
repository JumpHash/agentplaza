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
//! own thread (checks.rs is the same shape). The target is the step's `stand_in` (target.rs): the stand-in is the only
//! target in 8.1, and the web, terminal, desktop, simulator and device targets of 8.2–8.5 implement the same trait.

pub mod models;
pub mod run;
pub mod scenario;
pub mod target;

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::recipes::graph::StepState;
use crate::recipes::model::{USER_TEST, UserTest};
use crate::recipes::runs::CheckRow;
use crate::recipes::{Move, Report};
use crate::review_graph::user::by;
use crate::review_graph::{Finding, Verdict};
use crate::state::App;
use models::{GatewayModels, Models, PlanRequest, Roles};
use run::{Ctx, Failure, Timed, run_scenario};
use scenario::{Origin, Scenario, accessibility};
use target::Target;

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
pub fn execute(
    ctx: &Ctx,
    input: &Input,
    open: &mut dyn FnMut() -> Result<Box<dyn Target>, String>,
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
    let roles = Roles {
        planner: config.planner.clone(),
        driver: config.driver.clone(),
        escalate: config.escalate.clone(),
        checker: config.checker.clone(),
    };
    let Some(stand) = config.stand_in.as_ref() else {
        return Ok(finish(
            app,
            run,
            step,
            Settled {
                passed: false,
                findings: 0,
                commands: "no target".into(),
                summary: "no target: the step has no stand_in, so no scenario ran".into(),
            },
            now,
        ));
    };
    let gateway = app.providers.gateway();
    let models = GatewayModels {
        gateway: &gateway,
        roles: &roles,
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
    let mut open = || -> Result<Box<dyn Target>, String> {
        let target: Box<dyn Target> = Box::new(stand.open());
        Ok(target)
    };
    let execution = match execute(&ctx, &input, &mut open) {
        Ok(execution) => execution,
        Err(why) => {
            return Ok(finish(
                app,
                run,
                step,
                Settled {
                    passed: false,
                    findings: 0,
                    commands: "the user test".into(),
                    summary: format!("the user test did not run: {why}"),
                },
                now,
            ));
        }
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
    let summary = execution.summary();
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
