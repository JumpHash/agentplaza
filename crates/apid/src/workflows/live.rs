//! The production [`Plant`] (PLAN 21.1, 23): a job's commands run in its fork (a test fork on the job's pool, started on
//! first use and retired with the session), the tests and build commands are the project's own, a deploy goes through the
//! deploy path (`environments::request`, so a protected environment asks the owner), a secret is read from the vault, and
//! an approval is an item in Approvals.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use apz_testsel::map::{RUNNER_CARGO, RUNNER_NPM};
use apz_testsel::select::full_command;
use secrecy::ExposeSecret;
use serde_json::json;

use crate::base_recipe::{self, Command, Step as ForkStep, WORKSPACE};
use crate::base_vm::{Answer, Fork, Live};
use crate::checks;
use crate::deploy;
use crate::environments;
use crate::ephemeral::platform::Production;
use crate::placement;
use crate::project_memory;
use crate::project_stack::{self, Stack, Toolchain};
use crate::releases::builder;
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

/// The production effects of one session of a run. Its ephemeral environments are the hub's: `envs` is the hub's store
/// with the production ports.
pub struct LivePlant<'a> {
    pub app: &'a Arc<App>,
    pub info: RunInfo,
    pub stack: Stack,
    pub repo: String,
    pub rev: String,
    pub host: &'a Live<'a>,
    pub forks: &'a Mutex<BTreeMap<String, Fork<'a, Live<'a>>>>,
    pub envs: Hub<'a>,
}

impl<'a> LivePlant<'a> {
    fn start_fork(&self, runs_on: &str) -> Result<Fork<'a, Live<'a>>, String> {
        let volumes = base_recipe::volumes(&self.stack);
        let node = placement::base_node(self.app, &self.info.project, runs_on, &volumes)
            .ok_or_else(|| format!("no node in the pool {runs_on} has room for a fork"))?;
        let steps: Vec<ForkStep> = base_recipe::fork_steps(&self.stack, &self.repo, &self.rev);
        Fork::start(self.host, node, self.info.project.clone(), &steps)
    }

    fn deploy_status(&self, environment: &str, id: &str) -> Option<String> {
        self.app
            .environments
            .of(&self.info.project)
            .into_iter()
            .find(|e| e.name == environment)?
            .deploys
            .into_iter()
            .find(|d| d.id == id)
            .map(|d| d.status)
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
        if deployed.status == "waiting" {
            return Ok(format!(
                "{version} to {environment} waits for the owner's approval"
            ));
        }
        let platform = self
            .app
            .environments
            .of(project)
            .into_iter()
            .find(|e| e.name == environment)
            .is_some_and(|e| matches!(deploy::Settings::of(&e), Ok(Some(_))));
        let mut status = deployed.status.clone();
        let deadline = Instant::now() + Duration::from_secs(DEPLOY_WAIT_S);
        while platform && status == "recorded" && Instant::now() < deadline {
            std::thread::sleep(Duration::from_secs(2));
            status = self
                .deploy_status(environment, &deployed.id)
                .unwrap_or(status);
        }
        if status == "recorded" && platform {
            return Err(format!(
                "{version} to {environment} is still rolling out after {DEPLOY_WAIT_S} s"
            ));
        }
        if matches!(
            status.as_str(),
            "failed" | "refused" | "rolled_back" | "rolled back"
        ) {
            return Err(format!("{version} to {environment}: {status}"));
        }
        Ok(format!("{version} to {environment}: {status}"))
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

    fn approval(&self, run: &str, step: &str, message: &str) -> Result<(), String> {
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
        for (job, fork) in forks {
            if let Err(why) = fork.retire() {
                eprintln!("apid: the workflow fork of {job}: {why}");
            }
        }
    }
}

/// Runs one session of a workflow run in production: the run's walk from `ready` (the steps the session starts with) until
/// it settles, the forks retired, and then the run's end (its instances the run did not keep come down).
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
    match setup(app, &info) {
        Err(why) => {
            app.recipe_runs
                .close_external(run, Vec::new(), false, &why, apz_plaza::now_ms());
        }
        Ok((stack, repo, rev)) => {
            let host = Live(app.as_ref());
            let forks = Mutex::new(BTreeMap::new());
            let plant = LivePlant {
                app,
                info,
                stack,
                repo,
                rev,
                host: &host,
                forks: &forks,
                envs,
            };
            if let Err(why) = super::drive::walk(&app.recipe_runs, &plant, run, ready) {
                eprintln!("apid: workflow run {run}: {why}");
            }
            plant.retire();
            super::drive::finish(&app.recipe_runs, &plant.envs, run, apz_plaza::now_ms());
            app.bus.publish("recipe_run", &json!({ "run": run }));
            super::report_finished(app, run);
            return;
        }
    }
    super::drive::finish(&app.recipe_runs, &envs, run, apz_plaza::now_ms());
    app.bus.publish("recipe_run", &json!({ "run": run }));
    super::report_finished(app, run);
}

/// What a run's steps read from the repo: the project's stack, its repo and the revision its checks run at (the landed
/// revision of its change, or main's head).
fn setup(app: &App, info: &RunInfo) -> Result<(Stack, String, String), String> {
    let repo = app
        .projects
        .get(&info.project)
        .and_then(|p| p.repo)
        .ok_or_else(|| format!("{} has no repo yet", info.project))?;
    let stack = project_stack::read(app, &info.project)?
        .ok_or_else(|| format!("{} is not on main", project_stack::FILE))?;
    let rev = match &info.change {
        Some(change) => match project_memory::landed_revision(app, change) {
            Ok(rev) => rev,
            Err(_) => super::main_rev(app, &info.project)?,
        },
        None => super::main_rev(app, &info.project)?,
    };
    Ok((stack, repo, rev))
}
