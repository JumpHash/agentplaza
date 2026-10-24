//! The workflows' tests (PLAN 21, 23): the file's checks, the compile to a recipe, and the runs through the recipe engine
//! with a fake plant (the commands, the builds, the deploys, the approvals). The ephemeral environments are the hub's own
//! (`crate::ephemeral`) over its store and the ephemeral tests' fake platforms: a run's deploy, check and teardown go
//! through the code the hub runs, and the Kubernetes calls are recorded by the fake API server in order.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use apz_store::rusqlite::Connection;

use super::drive::{self, Artifact, Plant, RunInfo, artifacts_of};
use super::envs::{Envs, Hub};
use super::model::{Known, Work, Workflow};
use crate::base_recipe::{Command, WORKSPACE};
use crate::base_vm::Answer;
use crate::ephemeral::against::Verdict;
use crate::ephemeral::tests::{
    FakeKube, Fakes, MANIFESTS, Site, healthy, kube_env, kube_up_trail, lane,
};
use crate::ephemeral::{Instance, State, Store, UpRequest};
use crate::recipes::graph::Report;
use crate::recipes::runs::{RunState, Runs};

/// The digest of the build the fake plant makes for `manifests@linux-x86_64`.
const BUILT_DIGEST: &str =
    "sha256:0101010101010101010101010101010101010101010101010101010101010101";

/// The fake plant: its commands run in memory (a failing command is one whose argv names `fail_on`), its deploys and
/// approvals are recorded, and its concurrency is measured. Its ephemeral environments are the hub's over `store` and
/// the fake platforms in `ports`; `events` is the order of the calls the run made (the Kubernetes calls and the checks).
pub struct FakePlant {
    info: RunInfo,
    fail_on: Option<&'static str>,
    build_fails: bool,
    execs: Mutex<Vec<Vec<String>>>,
    active: AtomicUsize,
    peak: AtomicUsize,
    deploys: Mutex<Vec<(String, Option<String>)>>,
    approvals: Mutex<Vec<String>>,
    store: Store,
    ports: Fakes,
    events: Arc<Mutex<Vec<String>>>,
}

impl Default for FakePlant {
    fn default() -> FakePlant {
        FakePlant {
            info: RunInfo::default(),
            fail_on: None,
            build_fails: false,
            execs: Mutex::default(),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            deploys: Mutex::default(),
            approvals: Mutex::default(),
            store: Store::default(),
            ports: Fakes::new(kube_env(&[])),
            events: Arc::default(),
        }
    }
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

    /// A change's plant whose ephemeral environment is a Kubernetes instance on `cluster`, whose address is `site`.
    fn with_kube(change: &str, cluster: &FakeKube, site: &Site) -> FakePlant {
        let mut plant = FakePlant::for_change(change);
        plant.ports = Fakes::new(kube_env(&[("EPHEMERAL_URL", site.base.as_str())]));
        plant.ports.kube = Some(cluster.base.clone());
        plant.ports.blobs = vec![(BUILT_DIGEST.into(), MANIFESTS.as_bytes().to_vec())];
        // The build's blob is what deploys: the instance's own artifact is not used when the run built one.
        plant.ports.artifact = b"not the build".to_vec();
        plant
    }

    fn ran(&self) -> Vec<Vec<String>> {
        self.execs.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn hub(&self) -> Hub<'_> {
        Hub {
            store: &self.store,
            ports: &self.ports,
        }
    }
}

impl Envs for FakePlant {
    fn up(&self, req: &UpRequest) -> Result<Instance, String> {
        self.hub().up(req)
    }

    fn held(&self, run: &str) -> Vec<Instance> {
        self.hub().held(run)
    }

    fn down(&self, instance: &str) -> Result<(), String> {
        self.hub().down(instance)
    }

    fn against(
        &self,
        instance: &Instance,
        tests: &mut dyn FnMut(&str) -> Result<String, String>,
    ) -> Verdict {
        self.hub().against(instance, tests)
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
        if command.label == "against" {
            let target = command
                .argv
                .iter()
                .find(|a| a.starts_with("APZ_TARGET_URL="))
                .cloned()
                .unwrap_or_default();
            self.events
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(format!("against {target}"));
        }
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

    fn artifacts(&self) -> Result<Vec<Artifact>, String> {
        if self.build_fails {
            return Err("manifests@linux-x86_64 was not built: no pool has room".into());
        }
        Ok(vec![Artifact {
            target: "manifests@linux-x86_64".into(),
            digest: BUILT_DIGEST.into(),
        }])
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
        self
    }

    fn retire(&self) {}
}

/// Starts a run of `text` for `change`: its recipe's runs, and the steps it starts with.
fn start_run(text: &str, change: &str, runs: &Runs) -> (String, Vec<String>) {
    let workflow = Workflow::parse("ship", text).expect("the workflow parses");
    let recipe = workflow.compile().expect("the workflow compiles");
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
    (run, ready)
}

/// Starts a run of `text` for the plant's change and walks it, then ends it as the production driver does.
fn run_to_end(text: &str, plant: &FakePlant, change: &str) -> (Runs, String) {
    let runs = Runs::default();
    let (run, ready) = start_run(text, change, &runs);
    drive::walk(&runs, plant, &run, ready).expect("the run walks");
    drive::finish(&runs, plant, &run, 2);
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
environment = "pr"
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
    let cluster = FakeKube::start(healthy);
    let site = Site::start();
    let plant = FakePlant::with_kube("c7", &cluster, &site);
    let (runs, run) = run_to_end(SHIP, &plant, "c7");
    assert_eq!(
        runs.view(&run).unwrap().run.state,
        RunState::Passed,
        "{:?}",
        runs.view(&run).unwrap().run.checks
    );
    // Each step is a check of the run, in the order it ran.
    let order: Vec<String> = steps_checked(&runs, &run)
        .into_iter()
        .map(|(step, _)| step)
        .collect();
    assert_eq!(
        order,
        ["build:1", "test:1", "preview:1", "preview:2", "preview:3"]
    );
    // The instance is the change's, kept in the hub's store for the run, and it was torn down once by the teardown step.
    let held = plant.store.for_run(&run);
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].subject, lane("c7"));
    assert_eq!(held[0].expires_ms - held[0].created_ms, 30 * 60_000);
    assert_eq!(held[0].state, State::TornDown);
    // The check ran against the instance's address.
    let checks: Vec<String> = plant
        .ran()
        .into_iter()
        .filter(|argv| argv.iter().any(|a| a == "sh"))
        .map(|argv| argv.join(" "))
        .collect();
    assert!(
        checks
            .iter()
            .any(|c| c.contains(&format!("{}/health", site.base))),
        "{checks:?}"
    );
}

#[test]
fn a_failing_against_fails_the_run_and_still_takes_the_instance_down() {
    let cluster = FakeKube::start(healthy);
    let site = Site::start();
    let mut plant = FakePlant::with_kube("c8", &cluster, &site);
    plant.fail_on = Some("curl");
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
    let held = plant.store.for_run(&run);
    assert_eq!(held[0].state, State::TornDown);
    assert!(
        cluster
            .trail()
            .last()
            .is_some_and(|t| t == &format!("DELETE /api/v1/namespaces/{}", "apz-web-lane-c8")),
        "{:?}",
        cluster.trail()
    );
    let teardown = view.run.checks.iter().find(|c| c.step == "teardown");
    assert!(teardown.is_some_and(|c| c.passed && c.summary.contains(&held[0].id)));
}

#[test]
fn a_land_builds_deploys_to_kubernetes_checks_the_instance_and_tears_it_down_in_order() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&events);
    let cluster = FakeKube::start(move |r| {
        log.lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(format!("{} {}", r.method, r.path));
        healthy(r)
    });
    let site = Site::start();
    let mut plant = FakePlant::with_kube("c20", &cluster, &site);
    plant.events = Arc::clone(&events);
    plant.ports.secrets = vec![("API_KEY".into(), "s3cr3t-value".into())];
    let (runs, run) = run_to_end(SHIP, &plant, "c20");
    assert_eq!(
        runs.view(&run).unwrap().run.state,
        RunState::Passed,
        "{:?}",
        runs.view(&run).unwrap().run.checks
    );

    // The build recorded its artifact on the run, and the deploy used that blob (the instance's own artifact would fail).
    let built = artifacts_of(&runs, &run);
    assert_eq!(
        built.get("manifests@linux-x86_64").map(String::as_str),
        Some(BUILT_DIGEST)
    );

    // The namespace is created, applied and rolled out; the check runs against the instance; the namespace is deleted.
    let ns = "apz-web-lane-c20";
    let mut expected = kube_up_trail(ns);
    expected.push(format!("against APZ_TARGET_URL={}", site.base));
    expected.push(format!("DELETE /api/v1/namespaces/{ns}"));
    let got = events.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert_eq!(got, expected);
}

#[test]
fn a_build_that_cannot_be_made_fails_the_build_step_with_its_reason() {
    let mut plant = FakePlant::for_change("c30");
    plant.build_fails = true;
    let (runs, run) = run_to_end(SHIP, &plant, "c30");
    assert_eq!(runs.view(&run).unwrap().run.state, RunState::Failed);
    let build = runs
        .view(&run)
        .unwrap()
        .run
        .checks
        .into_iter()
        .find(|c| c.step == "build:1");
    assert!(build.is_some_and(|c| !c.passed && c.summary.contains("no pool has room")));
}

#[test]
fn a_teardown_after_a_restart_finds_the_instance_in_plat_db_and_takes_it_down() {
    let cluster = FakeKube::start(healthy);
    let site = Site::start();
    let gated = r#"
name = "ship"
triggers = ["land"]
[[jobs]]
id = "build"
[[jobs.steps]]
kind = "build"
[[jobs]]
id = "preview"
needs = ["build"]
[[jobs.steps]]
kind = "deploy"
environment = "pr"
ephemeral = true
[[jobs.steps]]
kind = "approval"
message = "check the preview?"
[[jobs.steps]]
kind = "against"
run = "curl -fsS ${{ env.url }}/health"
[[jobs.steps]]
kind = "teardown"
"#;
    let runs = Runs::default();
    // The first hub: the run deploys, then waits for the owner.
    let first = FakePlant::with_kube("c40", &cluster, &site);
    let (run, ready) = start_run(gated, "c40", &runs);
    drive::walk(&runs, &first, &run, ready).expect("the run walks to the approval");
    let approval = first.approvals.lock().unwrap().clone();
    assert_eq!(approval.len(), 1, "{approval:?}");
    assert_eq!(first.store.for_run(&run).len(), 1, "the instance is up");

    // The hub stops and starts again: its ephemeral store is read back from plaza.db.
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(&crate::ephemeral::store::def().ddl())
        .unwrap();
    apz_store::table::replace(
        &conn,
        crate::ephemeral::store::def(),
        &first.store.rows().unwrap(),
    )
    .unwrap();
    let reloaded = Store::default();
    reloaded.load(&conn).unwrap();
    let mut second = FakePlant::with_kube("c40", &cluster, &site);
    second.store = reloaded;

    // The owner approves; the run walks on on the new hub, and its teardown finds the instance.
    let now = 3;
    let moves = runs.settle(
        &run,
        &approval[0],
        &Report {
            passed: true,
            owner: Some(true),
            ..Report::default()
        },
        now,
    );
    let ready = moves
        .into_iter()
        .filter_map(|m| match m {
            crate::recipes::graph::Move::Ready(step) => Some(step),
            _ => None,
        })
        .collect();
    drive::walk(&runs, &second, &run, ready).expect("the run walks on");
    drive::finish(&runs, &second, &run, now);

    assert_eq!(runs.view(&run).unwrap().run.state, RunState::Passed);
    let held = second.store.for_run(&run);
    assert_eq!(
        held[0].state,
        State::TornDown,
        "the teardown found the instance"
    );
    assert!(
        cluster
            .trail()
            .last()
            .is_some_and(|t| t.starts_with("DELETE /api/v1/namespaces/")),
        "{:?}",
        cluster.trail()
    );
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
    assert!(plant.store.for_run(&run).is_empty());
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
fn the_run_info_round_trips_through_its_subject() {
    let info = RunInfo::of("web", Some("change:c3"));
    assert_eq!(info.change.as_deref(), Some("c3"));
    assert_eq!(info.subject().as_deref(), Some("change:c3"));
    let info = RunInfo::of("web", Some("release:2.0.1"));
    assert_eq!(info.release.as_deref(), Some("2.0.1"));
    assert!(RunInfo::of("web", None).subject().is_none());
}
