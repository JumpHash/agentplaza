//! The production [`Plant`] (PLAN 21.1, 23): a job's commands run in its fork (a test fork on the job's pool, started on
//! first use and retired with the session), the tests and build commands are the project's own, a deploy goes through the
//! deploy path (`environments::request`, so a protected environment asks the owner), a secret is read from the vault, and
//! an approval is an item in Approvals.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use apz_testsel::map::{RUNNER_CARGO, RUNNER_NPM};
use apz_testsel::select::full_command;
use secrecy::ExposeSecret;
use serde_json::json;

use crate::base_recipe::{self, Command, Step as ForkStep, WORKSPACE};
use crate::base_vm::{Answer, Fork, Live};
use crate::checks;
use crate::deploy;
use crate::environments::{self, Deploy};
use crate::ephemeral::platform::Production;
use crate::placement;
use crate::project_memory;
use crate::project_stack::{self, Stack, Toolchain};
use crate::recipes::runs::RunState;
use crate::releases::builder;
use crate::releases::entity::Release;
use crate::releases::flow::{Ledger, Outcome};
use crate::state::App;
use crate::targets;
use crate::test_select;
use crate::vault::{Consumer, OnBehalf};

use super::drive::{Artifact, Plant, RunInfo};
use super::envs::{Envs, Hub};
use super::model::Work;

/// How long a deploy to a platform may roll out before the step gives up on it.
const DEPLOY_WAIT_S: u64 = 1800;

/// How often a step reads its deploy's status while it waits for the outcome.
const DEPLOY_POLL_S: u64 = 2;

/// The job forks of one run, by job.
type Forks = BTreeMap<String, Fork<'static, Live<'static>>>;

/// The job forks of runs paused at an approval, kept for the session the owner's decision starts (by run). A fork's host is
/// the hub's for its life, so a fork outlives the session that started it. A restart drops them: their VMs stay on their
/// nodes, and the resumed job starts with a fresh fork.
static PAUSED: Mutex<BTreeMap<String, Forks>> = Mutex::new(BTreeMap::new());

/// The production effects of one session of a run. Its ephemeral environments are the hub's: `envs` is the hub's store
/// with the production ports.
pub struct LivePlant<'a> {
    pub app: &'a Arc<App>,
    pub info: RunInfo,
    pub stack: Stack,
    pub repo: String,
    pub rev: String,
    pub host: &'static Live<'static>,
    pub forks: &'a Mutex<Forks>,
    pub envs: Hub<'a>,
    /// Set when this session asked the owner about an approval: the run may then be paused, not settled.
    pub waiting: AtomicBool,
}

impl<'a> LivePlant<'a> {
    fn start_fork(&self, runs_on: &str) -> Result<Fork<'static, Live<'static>>, String> {
        let volumes = base_recipe::volumes(&self.stack);
        let node = placement::base_node(self.app, &self.info.project, runs_on, &volumes)
            .ok_or_else(|| format!("no node in the pool {runs_on} has room for a fork"))?;
        let steps: Vec<ForkStep> = base_recipe::fork_steps(&self.stack, &self.repo, &self.rev);
        Fork::start(self.host, node, self.info.project.clone(), &steps)
    }

    fn deploy_of(&self, environment: &str, id: &str) -> Option<Deploy> {
        self.app
            .environments
            .of(&self.info.project)
            .into_iter()
            .find(|e| e.name == environment)?
            .deploys
            .into_iter()
            .find(|d| d.id == id)
    }
}

impl<'a> Plant for LivePlant<'a> {
    fn info(&self) -> RunInfo {
        self.info.clone()
    }

    fn exec(&self, work: &Work, command: &Command) -> Result<Answer, String> {
        let taken = self
            .forks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&work.job);
        let mut fork = match taken {
            Some(fork) => fork,
            None => self.start_fork(&work.runs_on)?,
        };
        let answer = fork.exec(command);
        self.forks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(work.job.clone(), fork);
        answer
    }

    fn tests(&self, select: &str) -> Result<Vec<Command>, String> {
        let project = self.info.project.as_str();
        let env = base_recipe::cache_env(&self.stack);
        let rust = self.stack.toolchains.contains_key(&Toolchain::Rust);
        let node = self.stack.toolchains.contains_key(&Toolchain::Node);
        let full = if rust || !node {
            full_command(RUNNER_CARGO)
        } else {
            full_command(RUNNER_NPM)
        };
        let line = match (select, &self.info.change) {
            ("changed", Some(_)) => {
                let diff = checks::revision_diff(self.app, &self.rev)?;
                match test_select::select_for(
                    &self.app.test_select,
                    project,
                    &checks::touches(&diff),
                ) {
                    Some((_, selection)) if !selection.full => selection.command,
                    _ => full.to_string(),
                }
            }
            _ => full.to_string(),
        };
        Ok(line
            .split(" && ")
            .filter(|part| !part.trim().is_empty())
            .map(|part| Command {
                label: "tests".into(),
                argv: part.split_whitespace().map(str::to_string).collect(),
                cwd: WORKSPACE,
                timeout_s: base_recipe::STEP_TIMEOUT_S,
                env: env.clone(),
                listen: None,
            })
            .collect())
    }

    fn build(&self) -> Result<Vec<Command>, String> {
        let env = base_recipe::cache_env(&self.stack);
        Ok(base_recipe::build_commands(&self.stack)
            .into_iter()
            .map(|argv| Command {
                label: "build".into(),
                argv,
                cwd: WORKSPACE,
                timeout_s: base_recipe::STEP_TIMEOUT_S,
                env: env.clone(),
                listen: None,
            })
            .collect())
    }

    fn deploy(&self, environment: &str, version: Option<&str>) -> Result<String, String> {
        let project = self.info.project.as_str();
        let version = version
            .map(str::to_string)
            .or_else(|| self.info.release.clone())
            .or_else(|| deploy::newest(self.app, project).map(|r| r.version))
            .ok_or("no qualified, published release yet: name a version")?;
        let deployed = environments::request(
            self.app,
            project,
            environment,
            &version,
            0,
            "workflow",
            "a workflow's deploy step",
        )?;
        let platform = self
            .app
            .environments
            .of(project)
            .into_iter()
            .find(|e| e.name == environment)
            .is_some_and(|e| matches!(deploy::Settings::of(&e), Ok(Some(_))));
        // The step settles on the deploy's outcome: an owner's approval, a hook's answer or the end of the rollout. A deploy
        // still going when the deadline passes fails the step.
        let id = deployed.id.clone();
        let deadline = Instant::now() + Duration::from_secs(DEPLOY_WAIT_S);
        let (settled, done) = await_deploy(
            deployed,
            platform,
            deadline,
            Duration::from_secs(DEPLOY_POLL_S),
            || self.deploy_of(environment, &id),
        );
        if !done {
            return Err(format!(
                "{version} to {environment} is still {} after {DEPLOY_WAIT_S} s",
                settled.status
            ));
        }
        if outcome(&settled.status, platform) == Some(false) {
            let why = if settled.detail.is_empty() {
                settled.status.clone()
            } else {
                format!("{}: {}", settled.status, settled.detail)
            };
            return Err(format!("{version} to {environment}: {why}"));
        }
        Ok(format!("{version} to {environment}: {}", settled.status))
    }

    fn secret(&self, name: &str) -> Result<String, String> {
        let consumer = Consumer {
            binding: format!("project:{}", self.info.project),
            on_behalf: OnBehalf::Hub,
        };
        self.app
            .vault
            .resolve(name, &consumer, apz_plaza::now_ms())
            .map(|secret| secret.expose_secret().to_string())
            .map_err(|e| format!("secret {name}: {}", e.message()))
    }

    fn release_id(&self, project: &str, version: &str) -> Result<String, String> {
        release_at(self.app, project, version)
            .map(|r| r.id)
            .ok_or_else(|| format!("no release {version} in {project}"))
    }

    fn approval(&self, run: &str, step: &str, message: &str) -> Result<(), String> {
        self.waiting.store(true, Ordering::SeqCst);
        let subject = if message.trim().is_empty() {
            format!("{}: a workflow step waits for the owner", self.info.project)
        } else {
            format!("{}: {message}", self.info.project)
        };
        crate::approvals::submit(
            self.app,
            "workflow",
            &subject,
            json!({ "cid": 0, "project": self.info.project, "run": run, "step": step }),
        );
        Ok(())
    }

    fn envs(&self) -> &dyn Envs {
        &self.envs
    }

    fn artifacts(&self) -> Result<Vec<Artifact>, String> {
        let project = self.info.project.as_str();
        let placements = targets::build_plan(self.app, project)?;
        let mut out = Vec::new();
        for declared in self.stack.targets.iter().filter(|t| t.platform.is_some()) {
            let Some(platform) = declared.platform else {
                continue;
            };
            let target = format!(
                "{}@{}",
                declared.name,
                platform.name().replace("arm64", "aarch64")
            );
            match builder::build_target(
                self.host,
                project,
                &self.repo,
                &self.rev,
                &self.stack,
                &placements,
                &target,
            )? {
                Outcome::Built(built) => {
                    let hash = self.app.weft.upload(&built.bytes)?;
                    out.push(Artifact {
                        target,
                        digest: hash.to_string(),
                    });
                }
                Outcome::NotRun(why) => return Err(format!("{target} was not built: {why}")),
            }
        }
        Ok(out)
    }

    fn retire(&self) {
        let forks = std::mem::take(&mut *self.forks.lock().unwrap_or_else(PoisonError::into_inner));
        retire_forks(forks);
    }
}

/// Whether a deploy's status settles the step: `Some(true)` when it passed, `Some(false)` when it failed, and `None` while it
/// is still going (waiting for the owner, sending to its hook, rolling out). A plain environment's `recorded` is final, since
/// nothing reports on it after the record; on a platform `recorded` is the moment before its rollout starts. A status the
/// deploy module does not know is not an outcome either, so the step keeps waiting for one.
fn outcome(status: &str, platform: bool) -> Option<bool> {
    match status {
        "passed" | "sent" => Some(true),
        "recorded" if !platform => Some(true),
        "failed" | "refused" | "rolled_back" | "rolled back" => Some(false),
        _ => None,
    }
}

/// Waits for a deploy's outcome: `read` gives the deploy as it is now (`None` keeps the last read), polled every `every`.
/// Returns the deploy as it stands, and whether its status settled (`false` when `deadline` passed while it was still going).
fn await_deploy(
    deployed: Deploy,
    platform: bool,
    deadline: Instant,
    every: Duration,
    mut read: impl FnMut() -> Option<Deploy>,
) -> (Deploy, bool) {
    let mut current = deployed;
    loop {
        if outcome(&current.status, platform).is_some() {
            return (current, true);
        }
        if Instant::now() >= deadline {
            return (current, false);
        }
        std::thread::sleep(every);
        if let Some(latest) = read() {
            current = latest;
        }
    }
}

/// The hub's live host as the kept forks need it: `'static`. The hub is one App per process, so the first session's App is
/// the one it keeps.
fn live_host(app: &Arc<App>) -> &'static Live<'static> {
    static APP: OnceLock<Arc<App>> = OnceLock::new();
    static HOST: OnceLock<Live<'static>> = OnceLock::new();
    let shared: &'static App = APP.get_or_init(|| Arc::clone(app)).as_ref();
    HOST.get_or_init(|| Live(shared))
}

/// Retires job forks: each one's VM stops, and a failure is printed.
fn retire_forks(forks: Forks) {
    for (job, fork) in forks {
        if let Err(why) = fork.retire() {
            eprintln!("apid: the workflow fork of {job}: {why}");
        }
    }
}

/// Runs one session of a workflow run in production: the run's walk from `ready` (the steps the session starts with) until
/// it settles or pauses at an approval. A settled run's forks are retired and its end comes (its instances the run did not
/// keep come down). A run paused at an approval keeps its job forks for the session the decision starts, so the steps after
/// the approval find the files the steps before it made.
pub fn run_live(app: &Arc<App>, run: &str, ready: Vec<String>) {
    let Some(view) = app.recipe_runs.view(run) else {
        return;
    };
    let info = RunInfo::of(
        view.run.project.as_deref().unwrap_or_default(),
        view.run.subject.as_deref(),
    );
    let ports = Production { app };
    let envs = Hub {
        store: &app.ephemeral,
        ports: &ports,
    };
    let kept = PAUSED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(run)
        .unwrap_or_default();
    match setup(app, &info) {
        Err(why) => {
            retire_forks(kept);
            app.recipe_runs
                .close_external(run, Vec::new(), false, &why, apz_plaza::now_ms());
        }
        Ok((stack, repo, rev)) => {
            let host = live_host(app);
            let forks = Mutex::new(kept);
            let plant = LivePlant {
                app,
                info,
                stack,
                repo,
                rev,
                host,
                forks: &forks,
                envs,
                waiting: AtomicBool::new(false),
            };
            if let Err(why) = super::drive::walk(&app.recipe_runs, &plant, run, ready) {
                eprintln!("apid: workflow run {run}: {why}");
            }
            // A walk that stopped at an approval leaves the run going: its forks wait for the decision's session. Any other
            // end settles the run, and its forks are retired.
            let paused = plant.waiting.load(Ordering::SeqCst)
                && app
                    .recipe_runs
                    .view(run)
                    .is_some_and(|v| matches!(v.run.state, RunState::Running | RunState::Owner));
            if paused {
                let kept =
                    std::mem::take(&mut *forks.lock().unwrap_or_else(PoisonError::into_inner));
                PAUSED
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(run.to_string(), kept);
            } else {
                plant.retire();
            }
            super::drive::finish(&app.recipe_runs, &plant.envs, run, apz_plaza::now_ms());
            app.bus.publish("recipe_run", &json!({ "run": run }));
            if !paused {
                super::report_finished(app, run);
            }
            return;
        }
    }
    super::drive::finish(&app.recipe_runs, &envs, run, apz_plaza::now_ms());
    app.bus.publish("recipe_run", &json!({ "run": run }));
    super::report_finished(app, run);
}

/// What a run's steps read from the repo: the project's stack, its repo and the revision its checks run at (see
/// `checked_rev`).
fn setup(app: &App, info: &RunInfo) -> Result<(Stack, String, String), String> {
    let repo = app
        .projects
        .get(&info.project)
        .and_then(|p| p.repo)
        .ok_or_else(|| format!("{} has no repo yet", info.project))?;
    let stack = project_stack::read(app, &info.project)?
        .ok_or_else(|| format!("{} is not on main", project_stack::FILE))?;
    let rev = checked_rev(
        info,
        |change| project_memory::landed_revision(app, change),
        |version| release_at(app, &info.project, version).map(|r| r.from),
        || super::main_rev(app, &info.project),
    )?;
    Ok((stack, repo, rev))
}

/// The revision a run's checks and artifacts are at. A change's run is at the revision its change landed (main's head
/// while it has none). A release-triggered run is at the revision its release was cut from, so its checks, builds and
/// deploys name the same candidate: a release the hub does not have, or that names no revision, fails the run's setup
/// rather than testing main in its place. A manual run is at main's head.
fn checked_rev(
    info: &RunInfo,
    landed: impl FnOnce(&str) -> Result<String, String>,
    cut_from: impl FnOnce(&str) -> Option<String>,
    main: impl FnOnce() -> Result<String, String>,
) -> Result<String, String> {
    match (&info.change, &info.release) {
        (Some(change), _) => landed(change.as_str()).or_else(|_| main()),
        (None, Some(version)) => cut_from(version.as_str())
            .filter(|rev| !rev.is_empty())
            .ok_or_else(|| {
                format!(
                    "no release {version} of {} with a revision to run at",
                    info.project
                )
            }),
        (None, None) => main(),
    }
}

/// The release of a project at `version`: the newest one, when a version was cut twice.
fn release_at(app: &App, project: &str, version: &str) -> Option<Release> {
    app.releases
        .list(project)
        .into_iter()
        .find(|r| r.version == version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deploy_in(status: &str, detail: &str) -> Deploy {
        Deploy {
            id: "d1".into(),
            version: "1.2.0".into(),
            by: "workflow".into(),
            at_ms: 1,
            status: status.into(),
            detail: detail.into(),
            approval: None,
        }
    }

    #[test]
    fn only_a_settled_status_is_an_outcome() {
        for status in ["recorded", "rolling", "waiting", "sending", "cancelled"] {
            assert_eq!(outcome(status, true), None, "{status} on a platform");
        }
        assert_eq!(outcome("waiting", false), None);
        assert_eq!(outcome("sending", false), None);
        assert_eq!(outcome("recorded", false), Some(true));
        assert_eq!(outcome("passed", true), Some(true));
        assert_eq!(outcome("sent", false), Some(true));
        for status in ["failed", "refused", "rolled_back", "rolled back"] {
            assert_eq!(outcome(status, true), Some(false), "{status}");
        }
    }

    #[test]
    fn a_rollout_read_as_rolling_keeps_the_step_waiting_until_it_passes() {
        // The first read after the start is `rolling`: the step must not pass on it.
        let mut reads = vec![
            Some(deploy_in("rolling", "")),
            Some(deploy_in("passed", "")),
        ]
        .into_iter();
        let deadline = Instant::now() + Duration::from_secs(60);
        let (settled, done) = await_deploy(
            deploy_in("recorded", ""),
            true,
            deadline,
            Duration::ZERO,
            || reads.next().flatten(),
        );
        assert!(done);
        assert_eq!(settled.status, "passed");
        assert!(reads.next().is_none(), "the reads stop at the outcome");
    }

    #[test]
    fn a_failed_rollout_fails_with_its_reason_and_a_passed_deadline_settles_nothing() {
        let mut reads = vec![
            Some(deploy_in("rolling", "")),
            Some(deploy_in("rolled_back", "sign-in failed")),
        ]
        .into_iter();
        let deadline = Instant::now() + Duration::from_secs(60);
        let (settled, done) = await_deploy(
            deploy_in("rolling", ""),
            true,
            deadline,
            Duration::ZERO,
            || reads.next().flatten(),
        );
        assert!(done);
        assert_eq!(outcome(&settled.status, true), Some(false));
        assert_eq!(settled.detail, "sign-in failed");

        // Still going when the deadline has passed: the step fails on the timeout, with the status it was left in.
        let (left, done) = await_deploy(
            deploy_in("waiting", ""),
            true,
            Instant::now(),
            Duration::ZERO,
            || None,
        );
        assert!(!done);
        assert_eq!(left.status, "waiting");
    }

    #[test]
    fn a_plain_environments_recorded_deploy_passes_without_a_read() {
        let deadline = Instant::now() + Duration::from_secs(60);
        let (settled, done) = await_deploy(
            deploy_in("recorded", ""),
            false,
            deadline,
            Duration::ZERO,
            || panic!("a settled deploy is not read again"),
        );
        assert!(done);
        assert_eq!(settled.status, "recorded");
    }

    fn run_for(change: Option<&str>, release: Option<&str>) -> RunInfo {
        RunInfo {
            project: "shop".into(),
            change: change.map(str::to_string),
            release: release.map(str::to_string),
        }
    }

    #[test]
    fn a_release_run_is_at_the_revision_its_release_was_cut_from() {
        // Main has moved on since the cut: the run takes the cut, not main's head.
        let rev = checked_rev(
            &run_for(None, Some("1.2.0")),
            |_| panic!("a release run has no change"),
            |version| (version == "1.2.0").then(|| "cut-rev".to_string()),
            || Ok("main-head".to_string()),
        );
        assert_eq!(rev, Ok("cut-rev".to_string()));
    }

    #[test]
    fn a_release_without_a_revision_fails_the_run_rather_than_testing_main() {
        for cut in [None, Some(String::new())] {
            let rev = checked_rev(
                &run_for(None, Some("9.9.9")),
                |_| panic!("a release run has no change"),
                |_| cut.clone(),
                || panic!("main is not the release's revision"),
            );
            let why = rev.unwrap_err();
            assert!(why.contains("9.9.9"), "{why}");
        }
    }

    #[test]
    fn a_change_run_is_at_its_landed_revision_or_main_while_it_has_none() {
        let landed = checked_rev(
            &run_for(Some("ch-4"), None),
            |change| {
                assert_eq!(change, "ch-4");
                Ok("landed-rev".to_string())
            },
            |_| panic!("a change run has no release"),
            || Ok("main-head".to_string()),
        );
        assert_eq!(landed, Ok("landed-rev".to_string()));
        let waiting = checked_rev(
            &run_for(Some("ch-4"), None),
            |_| Err("no landing yet".to_string()),
            |_| panic!("a change run has no release"),
            || Ok("main-head".to_string()),
        );
        assert_eq!(waiting, Ok("main-head".to_string()));
    }

    #[test]
    fn a_manual_run_is_at_main() {
        let rev = checked_rev(
            &run_for(None, None),
            |_| panic!("a manual run has no change"),
            |_| panic!("a manual run has no release"),
            || Ok("main-head".to_string()),
        );
        assert_eq!(rev, Ok("main-head".to_string()));
    }
}
