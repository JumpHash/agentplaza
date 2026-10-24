//! The workflows' tests (PLAN 21, 23): the file's checks, the compile to a recipe, and the runs through the recipe engine with a
//! fake plant (the commands, the deploys, the approvals) and fake ephemeral environments.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::drive::{self, Plant, RunInfo};
use super::envs::{Envs, Instance, Subject, UpRequest};
use super::model::{Known, Work, Workflow};
use crate::base_recipe::{Command, WORKSPACE};
use crate::base_vm::Answer;
use crate::recipes::graph::Report;
use crate::recipes::runs::{RunState, Runs};

/// The fake ephemeral environments: what was brought up, taken down and checked.
#[derive(Default)]
struct FakeEnvs {
    ups: Mutex<Vec<UpRequest>>,
    downs: Mutex<Vec<String>>,
    checks: Mutex<Vec<String>>,
    check_fails: bool,
    down_fails: bool,
}

impl Envs for FakeEnvs {
    fn up(&self, req: &UpRequest) -> Result<Instance, String> {
        let mut ups = self.ups.lock().unwrap_or_else(|e| e.into_inner());
        ups.push(req.clone());
        Ok(Instance {
            id: format!("eph-{}", ups.len()),
            project: req.project.clone(),
            environment: req.environment.clone(),
            subject: req.subject.clone(),
            target: "fake".into(),
            url: Some("https://preview.test".into()),
            state: "up".into(),
            created_ms: 1,
            expires_ms: 2,
        })
    }

    fn down(&self, instance: &Instance) -> Result<(), String> {
        if self.down_fails {
            return Err("the environment would not go down".into());
        }
        self.downs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(instance.id.clone());
        Ok(())
    }

    fn against(&self, _instance: &Instance, command: &str) -> Result<String, String> {
        self.checks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(command.to_string());
        if self.check_fails {
            Err("the checks failed at the preview".into())
        } else {
            Ok("checks passed".into())
        }
    }
}

/// The fake plant: its commands run in memory (a failing command is one whose argv names `fail_on`), its deploys and
/// approvals are recorded, and its concurrency is measured.
#[derive(Default)]
struct FakePlant {
    info: RunInfo,
    fail_on: Option<&'static str>,
    execs: Mutex<Vec<Vec<String>>>,
    active: AtomicUsize,
    peak: AtomicUsize,
    deploys: Mutex<Vec<(String, Option<String>)>>,
    approvals: Mutex<Vec<String>>,
    envs: FakeEnvs,
}

impl FakePlant {
    fn for_change(change: &str) -> FakePlant {
        FakePlant {
            info: RunInfo {
                project: "web".into(),
                change: Some(change.into()),
                release: None,
            },
            ..FakePlant::default()
        }
    }

    fn ran(&self) -> Vec<Vec<String>> {
        self.execs.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

fn command(label: &str, line: &str) -> Command {
    Command {
        label: label.into(),
        argv: line.split_whitespace().map(str::to_string).collect(),
        cwd: WORKSPACE,
        timeout_s: 60,
        env: Vec::new(),
        listen: None,
    }
}

impl Plant for FakePlant {
    fn info(&self) -> RunInfo {
        self.info.clone()
    }

    fn exec(&self, _work: &Work, command: &Command) -> Result<Answer, String> {
        let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(30));
        self.active.fetch_sub(1, Ordering::SeqCst);
        self.execs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(command.argv.clone());
        let fails = self
            .fail_on
            .is_some_and(|needle| command.argv.iter().any(|a| a.contains(needle)));
        Ok(Answer {
            ok: !fails,
            exit_code: Some(if fails { 1 } else { 0 }),
            output: format!("ran {}", command.argv.join(" ")),
        })
    }

    fn tests(&self, _select: &str) -> Result<Vec<Command>, String> {
        Ok(vec![command("tests", "cargo test -p apid")])
    }

    fn build(&self) -> Result<Vec<Command>, String> {
        Ok(vec![command("build", "cargo build")])
    }

    fn deploy(&self, environment: &str, version: Option<&str>) -> Result<String, String> {
        self.deploys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((environment.to_string(), version.map(str::to_string)));
        Ok(format!("deployed to {environment}"))
    }

    fn secret(&self, name: &str) -> Result<String, String> {
        if name == "NPM_TOKEN" {
            Ok("s3cret-value".into())
        } else {
            Err(format!("secret {name}: not in the vault"))
        }
    }

    fn approval(&self, _run: &str, step: &str, _message: &str) -> Result<(), String> {
        self.approvals
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(step.to_string());
        Ok(())
    }

    fn envs(&self) -> &dyn Envs {
        &self.envs
    }

    fn retire(&self) {}
}

/// The walks of these tests share the process's ephemeral-instance table, so they run one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

/// Starts a run of `text` for the plant's change and walks it, then ends it as the production driver does.
fn run_to_end(text: &str, plant: &FakePlant, change: &str) -> (Runs, String) {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drive::forget_all();
    let workflow = Workflow::parse("ship", text).expect("the workflow parses");
    let recipe = workflow.compile().expect("the workflow compiles");
    let runs = Runs::default();
    let info = RunInfo::of("web", Some(&format!("change:{change}")));
    let (run, moves) = runs.start(
        &recipe,
        "workflow",
        "land",
        None,
        Some("web"),
        info.subject().as_deref(),
        1,
    );
    let ready = moves
        .into_iter()
        .filter_map(|m| match m {
            crate::recipes::graph::Move::Ready(step) => Some(step),
            _ => None,
        })
        .collect();
    drive::walk(&runs, plant, &run, ready).expect("the run walks");
    drive::finish(&runs, &plant.envs, &run, 2);
    (runs, run)
}

fn steps_checked(runs: &Runs, run: &str) -> Vec<(String, bool)> {
    runs.view(run)
        .expect("the run is listed")
        .run
        .checks
        .iter()
        .map(|c| (c.step.clone(), c.passed))
        .collect()
}

const SHIP: &str = r#"
name = "ship"
triggers = ["land", "manual"]

[[jobs]]
id = "build"
[[jobs.steps]]
kind = "build"

[[jobs]]
id = "test"
needs = ["build"]
[[jobs.steps]]
kind = "test"
select = "changed"

[[jobs]]
id = "preview"
needs = ["test"]
[[jobs.steps]]
kind = "deploy"
environment = "preview"
ephemeral = true
ttl_minutes = 30
[[jobs.steps]]
kind = "against"
run = "curl -fsS ${{ env.url }}/health"
[[jobs.steps]]
kind = "teardown"
"#;

#[test]
fn a_workflow_file_parses_and_refuses_what_is_wrong_with_a_clear_reason() {
    assert!(Workflow::parse("ship", SHIP).is_ok());

    // An unknown step kind is named.
    let text = SHIP.replace("kind = \"build\"", "kind = \"bake\"");
    let why = Workflow::parse("ship", &text).unwrap_err();
    assert!(why.contains("bake"), "{why}");

    // A cycle in needs is refused, and the jobs in it are named.
    let text = SHIP.replace("id = \"build\"", "id = \"build\"\nneeds = [\"preview\"]");
    let why = Workflow::parse("ship", &text).unwrap_err();
    assert!(why.contains("cycle"), "{why}");

    // A need of a job that is not there is refused.
    let text = SHIP.replace("needs = [\"test\"]", "needs = [\"nowhere\"]");
    assert!(
        Workflow::parse("ship", &text)
            .unwrap_err()
            .contains("nowhere")
    );

    // An unknown environment is refused against the project's environments.
    let known = Known {
        environments: Some(vec!["staging".into()]),
        pools: None,
    };
    let why = Workflow::parse("ship", SHIP)
        .unwrap()
        .check(&known)
        .unwrap_err();
    assert!(
        why.contains("preview") && why.contains("environments"),
        "{why}"
    );

    // A runs_on the stack does not declare is refused.
    let text = SHIP.replace("id = \"build\"", "id = \"build\"\nruns_on = \"arm64-hvf\"");
    let known = Known {
        environments: None,
        pools: Some(vec!["any".into(), "x86_64-kvm".into()]),
    };
    assert!(
        Workflow::parse("ship", &text)
            .unwrap()
            .check(&known)
            .unwrap_err()
            .contains("arm64-hvf")
    );

    // An expression that is not one of the runs' is refused; a secret outside an env value is refused.
    let text = SHIP.replace("${{ env.url }}", "${{ env.port }}");
    assert!(
        Workflow::parse("ship", &text)
            .unwrap_err()
            .contains("env.port")
    );
    let text = SHIP.replace(
        "kind = \"build\"",
        "kind = \"run\"\nrun = \"echo ${{ secrets.NPM_TOKEN }}\"",
    );
    assert!(
        Workflow::parse("ship", &text)
            .unwrap_err()
            .contains("secret")
    );

    // A schedule needs its cron; a cron needs the schedule trigger.
    let text = SHIP.replace(
        "triggers = [\"land\", \"manual\"]",
        "triggers = [\"schedule\"]",
    );
    assert!(Workflow::parse("ship", &text).unwrap_err().contains("cron"));
    let text = format!("{SHIP}\n").replace(
        "triggers = [\"land\", \"manual\"]",
        "triggers = [\"land\"]\ncron = \"0 3 * * *\"",
    );
    assert!(
        Workflow::parse("ship", &text)
            .unwrap_err()
            .contains("schedule")
    );

    // A trigger that is not one is refused.
    let text = SHIP.replace("\"land\", \"manual\"", "\"tuesday\"");
    assert!(
        Workflow::parse("ship", &text)
            .unwrap_err()
            .contains("tuesday")
    );
}

#[test]
fn a_matrix_expands_into_parallel_jobs_and_a_need_waits_for_all_of_them() {
    let text = r#"
name = "matrix"
triggers = ["manual"]
[[jobs]]
id = "test"
matrix = { os = ["linux", "mac"], node = [18, 20] }
[[jobs.steps]]
kind = "run"
run = "npm test # ${{ matrix.os }} ${{ matrix.node }}"
[[jobs]]
id = "release"
needs = ["test"]
[[jobs.steps]]
kind = "run"
run = "echo released"
"#;
    let workflow = Workflow::parse("matrix", text).unwrap();
    let expanded = workflow.expand();
    let names: Vec<&str> = expanded.iter().map(|j| j.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "test[node=18,os=linux]",
            "test[node=18,os=mac]",
            "test[node=20,os=linux]",
            "test[node=20,os=mac]",
            "release",
        ]
    );
    let recipe = workflow.compile().unwrap();
    // The release waits for all four test jobs: four edges into its first step.
    let release_first = "release:1";
    let into_release = recipe
        .edges
        .iter()
        .filter(|e| e.to == release_first)
        .count();
    assert_eq!(into_release, 4);

    // The four run in parallel: all four are ready when the run starts, and their commands overlap.
    let plant = FakePlant::for_change("c1");
    let (runs, run) = run_to_end(text, &plant, "c1");
    assert_eq!(runs.view(&run).unwrap().run.state, RunState::Passed);
    let mut checks = steps_checked(&runs, &run);
    checks.sort();
    assert_eq!(checks.len(), 5);
    assert!(checks.iter().all(|(_, passed)| *passed));
    assert!(
        plant.peak.load(Ordering::SeqCst) >= 2,
        "the matrix jobs overlapped"
    );
}

#[test]
fn a_land_runs_build_test_deploy_against_teardown_in_order_and_takes_the_instance_down() {
    let plant = FakePlant::for_change("c7");
    let (runs, run) = run_to_end(SHIP, &plant, "c7");
    assert_eq!(runs.view(&run).unwrap().run.state, RunState::Passed);
    // Each step is a check of the run, in the order it ran.
    let order: Vec<String> = steps_checked(&runs, &run)
        .into_iter()
        .map(|(step, _)| step)
        .collect();
    assert_eq!(
        order,
        ["build:1", "test:1", "preview:1", "preview:2", "preview:3"]
    );
    // The ephemeral instance is for the change, the check ran against its URL, and the teardown took it down once.
    let ups = plant.envs.ups.lock().unwrap().clone();
    assert_eq!(ups.len(), 1);
    assert_eq!(ups[0].subject, Subject::Lane("c7".into()));
    assert_eq!(ups[0].ttl_minutes, Some(30));
    let checks = plant.envs.checks.lock().unwrap().clone();
    assert_eq!(
        checks,
        vec!["curl -fsS https://preview.test/health".to_string()]
    );
    assert_eq!(plant.envs.downs.lock().unwrap().len(), 1);
}

#[test]
fn a_failing_against_fails_the_run_and_still_takes_the_instance_down() {
    let mut plant = FakePlant::for_change("c8");
    plant.envs.check_fails = true;
    let (runs, run) = run_to_end(SHIP, &plant, "c8");
    let view = runs.view(&run).unwrap();
    assert_eq!(view.run.state, RunState::Failed);
    let steps = steps_checked(&runs, &run);
    assert!(
        steps.contains(&("preview:2".to_string(), false)),
        "{steps:?}"
    );
    // The teardown step after it never ran, and the run's end took the instance down anyway.
    assert!(!steps.iter().any(|(step, _)| step == "preview:3"));
    assert_eq!(plant.envs.downs.lock().unwrap().len(), 1);
    let teardown = view.run.checks.iter().find(|c| c.step == "teardown");
    assert!(teardown.is_some_and(|c| c.passed && c.summary.contains("eph-1")));
}

#[test]
fn a_failed_step_fails_its_job_and_the_jobs_that_need_it() {
    let mut plant = FakePlant::for_change("c9");
    plant.fail_on = Some("cargo");
    let (runs, run) = run_to_end(SHIP, &plant, "c9");
    let view = runs.view(&run).unwrap();
    assert_eq!(view.run.state, RunState::Failed);
    let steps = steps_checked(&runs, &run);
    assert_eq!(steps, vec![("build:1".to_string(), false)]);
    // Nothing after the failure ran: no deploy was brought up.
    assert!(plant.envs.ups.lock().unwrap().is_empty());
    assert!(
        !view
            .run
            .checks
            .iter()
            .any(|c| c.step == "teardown" && c.summary.contains("eph"))
    );
}

#[test]
fn an_approval_waits_for_the_owner_and_resumes_when_it_is_given() {
    let text = r#"
name = "gate"
triggers = ["manual"]
[[jobs]]
id = "build"
[[jobs.steps]]
kind = "build"
[[jobs]]
id = "ship"
needs = ["build"]
[[jobs.steps]]
kind = "approval"
message = "ship to production"
[[jobs.steps]]
kind = "run"
run = "echo shipped"
"#;
    let workflow = Workflow::parse("gate", text).unwrap();
    let recipe = workflow.compile().unwrap();
    let plant = FakePlant {
        info: RunInfo {
            project: "web".into(),
            change: None,
            release: None,
        },
        ..FakePlant::default()
    };
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    drive::forget_all();
    let runs = Runs::default();
    let (run, moves) = runs.start(&recipe, "workflow", "manual", None, Some("web"), None, 1);
    let ready = |moves: Vec<crate::recipes::graph::Move>| -> Vec<String> {
        moves
            .into_iter()
            .filter_map(|m| match m {
                crate::recipes::graph::Move::Ready(step) => Some(step),
                _ => None,
            })
            .collect()
    };
    drive::walk(&runs, &plant, &run, ready(moves)).unwrap();
    // The walk stopped at the approval: it was asked, and the run is still going.
    assert_eq!(
        plant.approvals.lock().unwrap().clone(),
        vec!["ship:1".to_string()]
    );
    assert_eq!(runs.view(&run).unwrap().run.state, RunState::Running);
    assert!(
        plant
            .ran()
            .iter()
            .all(|argv| argv.iter().all(|a| a != "echo shipped"))
    );

    // The owner's approval settles the step, and the walk goes on to the run after it.
    let next = runs.settle(
        &run,
        "ship:1",
        &Report {
            passed: true,
            owner: Some(true),
            ..Report::default()
        },
        3,
    );
    drive::walk(&runs, &plant, &run, ready(next)).unwrap();
    assert_eq!(runs.view(&run).unwrap().run.state, RunState::Passed);
    assert!(
        plant
            .ran()
            .iter()
            .any(|argv| argv.iter().any(|a| a == "echo shipped"))
    );
}

#[test]
fn a_secret_reaches_the_command_but_not_the_run_record() {
    let text = r#"
name = "npm"
triggers = ["manual"]
[[jobs]]
id = "publish"
[[jobs.steps]]
kind = "run"
run = "npm publish"
env = { NPM_TOKEN = "${{ secrets.NPM_TOKEN }}", CI = "1" }
"#;
    let plant = FakePlant::default();
    let (runs, run) = run_to_end(text, &plant, "c1");
    assert_eq!(runs.view(&run).unwrap().run.state, RunState::Passed);
    let ran = plant.ran();
    assert!(
        ran.iter()
            .any(|argv| argv.contains(&"NPM_TOKEN=s3cret-value".to_string())),
        "{ran:?}"
    );
    let record = format!("{:?}", runs.view(&run).unwrap().run.checks);
    assert!(
        !record.contains("s3cret-value"),
        "the record shows no secret"
    );
}

#[test]
fn an_ephemeral_deploy_needs_a_change_or_a_release() {
    let plant = FakePlant::default();
    let (runs, run) = run_to_end(SHIP, &plant, "c1");
    // Without a change or a release (a manual run of the workflow), the deploy fails and says why.
    let view = runs.view(&run).unwrap();
    assert_eq!(view.run.state, RunState::Failed);
    let deploy = view.run.checks.iter().find(|c| c.step == "preview:1");
    assert!(deploy.is_some_and(|c| !c.passed && c.summary.contains("change or a release")));
}

#[test]
fn a_named_deploy_and_a_promote_go_through_the_plant() {
    let text = r#"
name = "release"
triggers = ["release.candidate"]
[[jobs]]
id = "ship"
[[jobs.steps]]
kind = "promote"
environment = "production"
[[jobs.steps]]
kind = "deploy"
environment = "staging"
version = "${{ release.version }}"
"#;
    let plant = FakePlant {
        info: RunInfo {
            project: "web".into(),
            change: None,
            release: Some("1.4.0".into()),
        },
        ..FakePlant::default()
    };
    let workflow = Workflow::parse("release", text).unwrap();
    let recipe = workflow.compile().unwrap();
    let runs = Runs::default();
    let (run, moves) = runs.start(
        &recipe,
        "workflow",
        "release.candidate",
        None,
        Some("web"),
        Some("release:1.4.0"),
        1,
    );
    let ready: Vec<String> = moves
        .into_iter()
        .filter_map(|m| match m {
            crate::recipes::graph::Move::Ready(step) => Some(step),
            _ => None,
        })
        .collect();
    drive::walk(&runs, &plant, &run, ready).unwrap();
    assert_eq!(runs.view(&run).unwrap().run.state, RunState::Passed);
    let deploys = plant.deploys.lock().unwrap().clone();
    assert_eq!(
        deploys,
        vec![
            ("production".to_string(), Some("1.4.0".to_string())),
            ("staging".to_string(), Some("1.4.0".to_string())),
        ]
    );
}

#[test]
fn the_production_envs_refuse_until_the_hub_wires_them() {
    use super::envs::{HubEnvs, NOT_WIRED};
    let req = UpRequest {
        project: "web".into(),
        environment: "preview".into(),
        subject: Subject::Lane("c1".into()),
        ttl_minutes: None,
    };
    assert_eq!(HubEnvs.up(&req).unwrap_err(), NOT_WIRED);
}

#[test]
fn the_run_info_round_trips_through_its_subject() {
    let info = RunInfo::of("web", Some("change:c3"));
    assert_eq!(info.change.as_deref(), Some("c3"));
    assert_eq!(info.subject().as_deref(), Some("change:c3"));
    let info = RunInfo::of("web", Some("release:2.0.1"));
    assert_eq!(info.release.as_deref(), Some("2.0.1"));
    assert!(RunInfo::of("web", None).subject().is_none());
}
