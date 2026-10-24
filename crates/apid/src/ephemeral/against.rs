//! The `against` stage: a change's tests run against an ephemeral instance of it (the CI recipe's step, and any caller that
//! asks). The instance is deployed (`super::reserve` and `super::execute`), the environment's checks are read from its
//! address (`EPHEMERAL_CHECK_PATHS`, `/` by default: each must answer 2xx), the tests run with the address in
//! `APZ_TARGET_URL`, and the project's confirmed user-test scenarios run through a web target at that address (8.2, the same
//! smoke set a deploy runs). The instance is torn down after the stage unless the caller keeps it. Each line is the stage's
//! evidence: what was probed, what the tests said, and whether the instance was torn down.
//!
//! The CI step runs its tests in the change's test fork (`checks::Exec`), so the address is reached from the fork; the
//! checks and scenarios are the hub's.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Serialize;

use crate::base_recipe::Command;
use crate::checks::Exec;
use crate::environments;
use crate::platforms::http::{Http, Ureq};
use crate::project_memory;
use crate::state::App;
use crate::user_test::scenario::Scenario;

use super::platform::Production;
use super::{Instance, Ports, Store, Subject, UpRequest, down_with};

/// The variable a test reads the instance's address from.
pub const TARGET_ENV: &str = "APZ_TARGET_URL";
/// The environment's variable that names the paths probed on the instance (comma-separated; `/` when unset).
const CHECK_PATHS: &str = "EPHEMERAL_CHECK_PATHS";
/// The most of a failing command's output a line keeps (its end).
const TAIL: usize = 1200;

/// What a stage asks for: the environment and subject, the paths probed, whether the instance is kept after, and the
/// minutes it lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub project: String,
    pub environment: String,
    pub subject: Subject,
    pub paths: Vec<String>,
    pub keep: bool,
    pub ttl_minutes: Option<u32>,
}

/// The stage's result: whether it passed, its evidence lines, and the instance as it is after the stage (None when it
/// never came up).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Verdict {
    pub passed: bool,
    pub lines: Vec<String>,
    pub instance: Option<Instance>,
}

/// Runs the stage's checks over an instance that `deployed` brought up: probes its paths, runs `tests` with its address
/// (the tests' own result is their line), and tears it down unless `req.keep`. An instance that did not come up fails the
/// stage with the reason; a failed teardown is a line, not a failure of the tests.
pub fn run(
    store: &Store,
    ports: &dyn Ports,
    req: &Request,
    deployed: Result<Instance, String>,
    tests: &mut dyn FnMut(&str) -> Result<String, String>,
) -> Verdict {
    let instance = match deployed {
        Ok(instance) => instance,
        Err(why) => {
            return Verdict {
                passed: false,
                lines: vec![format!("the instance did not come up: {why}")],
                instance: None,
            };
        }
    };
    let id = instance.id.clone();
    let mut passed = true;
    let mut lines = Vec::new();
    match instance.url.clone().filter(|u| !u.is_empty()) {
        None => {
            passed = false;
            lines.push("the instance has no address to test against".to_string());
        }
        Some(base) => {
            for path in &req.paths {
                match probe(&base, path) {
                    Ok(status) => lines.push(format!("GET {path}: {status}")),
                    Err(why) => {
                        passed = false;
                        lines.push(format!("GET {path}: {why}"));
                    }
                }
            }
            match tests(&base) {
                Ok(summary) => lines.push(summary),
                Err(why) => {
                    passed = false;
                    lines.push(format!("tests: {why}"));
                }
            }
        }
    }
    let instance = if req.keep {
        lines.push(format!("kept: {id} stays until its time"));
        instance
    } else {
        match down_with(store, ports, &id) {
            Ok(()) => {
                lines.push(format!("torn down: {id}"));
                store.get(&id).unwrap_or(instance)
            }
            Err(why) => {
                lines.push(format!("not torn down: {why}"));
                instance
            }
        }
    };
    Verdict {
        passed,
        lines,
        instance: Some(instance),
    }
}

/// One GET of `path` under `base`: its status when it answers 2xx, else why not.
fn probe(base: &str, path: &str) -> Result<u16, String> {
    let url = format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
    let reply = Ureq.call("GET", &url, &[], None)?;
    if (200..300).contains(&reply.status) {
        Ok(reply.status)
    } else {
        Err(format!("answered {}", reply.status))
    }
}

/// The paths an environment's instances are probed on.
pub fn paths_of(env: &environments::Environment) -> Vec<String> {
    let paths: Vec<String> = crate::deploy::variable(env, CHECK_PATHS)
        .unwrap_or_else(|| "/".to_string())
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect();
    if paths.is_empty() {
        vec!["/".to_string()]
    } else {
        paths
    }
}

/// The CI recipe's `against` step (the production entry): the project's first ephemeral environment gets an instance of
/// the change, the change's tests run in its fork with the address in `APZ_TARGET_URL`, and the instance is torn down. A
/// project with no ephemeral environment passes with a note. Returns the stage's evidence, or why it failed.
pub fn ci(
    app: &Arc<App>,
    project: &str,
    change: &str,
    tests: &[Command],
    exec: &mut Exec<'_>,
) -> Result<String, String> {
    let Some(env) = environments::list(app, project)
        .into_iter()
        .find(super::is_ephemeral)
    else {
        return Ok("no ephemeral environment: nothing to run against".to_string());
    };
    let subject = Subject::Lane(change.to_string());
    let req = Request {
        project: project.to_string(),
        environment: env.name.clone(),
        subject: subject.clone(),
        paths: paths_of(&env),
        keep: false,
        ttl_minutes: None,
    };
    let deployed = super::up(
        app,
        UpRequest {
            project: project.to_string(),
            environment: env.name.clone(),
            subject,
            ttl_minutes: None,
            run: None,
            keep: false,
            built: BTreeMap::new(),
        },
    );
    let commit = project_memory::landed_revision(app, change)?;
    let ports = Production { app };
    let verdict = run(&app.ephemeral, &ports, &req, deployed, &mut |url| {
        let mut lines = Vec::new();
        for command in tests {
            let mut argv = vec!["env".to_string(), format!("{TARGET_ENV}={url}")];
            argv.extend(command.argv.iter().cloned());
            let on = Command {
                label: command.label.clone(),
                argv,
                cwd: command.cwd,
                timeout_s: command.timeout_s,
                env: command.env.clone(),
                listen: None,
            };
            match exec(&on) {
                Ok(answer) if answer.ok => lines.push(format!("{}: passed", command.label)),
                Ok(answer) => {
                    return Err(format!(
                        "{} against {url}: {}",
                        command.label,
                        tail(answer.output.trim_end())
                    ));
                }
                Err(why) => return Err(format!("{} against {url}: {why}", command.label)),
            }
        }
        let scenarios = confirmed_scenarios(app, project);
        if !scenarios.is_empty() {
            crate::user_test::smoke(app, project, url, &commit, &scenarios)?;
            lines.push(format!("{} user-test scenario(s) passed", scenarios.len()));
        }
        Ok(lines.join("; "))
    });
    let summary = format!("{}; {}", verdict.lines.join("; "), env.name);
    if verdict.passed {
        Ok(summary)
    } else {
        Err(summary)
    }
}

/// The owner's route (`POST /v1/projects/{id}/ephemeral/against`): deploys an instance of the request, probes its paths and
/// runs the project's confirmed user-test scenarios against its address, then tears it down unless the request keeps it.
/// Empty `paths` probe the environment's own. A lane's scenarios run at its landed revision, a release's at its own.
pub fn check(app: &Arc<App>, req: &Request) -> Result<Verdict, String> {
    let env = environments::list(app, &req.project)
        .into_iter()
        .find(|e| e.name == req.environment)
        .ok_or_else(|| format!("no environment {}", req.environment))?;
    let paths = if req.paths.is_empty() {
        paths_of(&env)
    } else {
        req.paths.clone()
    };
    let commit = match &req.subject {
        Subject::Lane(change) => project_memory::landed_revision(app, change)?,
        Subject::Release(id) => app
            .releases
            .list(&req.project)
            .into_iter()
            .find(|r| &r.id == id)
            .map(|r| r.from)
            .ok_or_else(|| format!("no release {id} in {}", req.project))?,
    };
    let deployed = super::up(
        app,
        UpRequest {
            project: req.project.clone(),
            environment: req.environment.clone(),
            subject: req.subject.clone(),
            ttl_minutes: req.ttl_minutes,
            run: None,
            keep: false,
            built: BTreeMap::new(),
        },
    );
    let req = Request {
        paths,
        ..req.clone()
    };
    let ports = Production { app };
    let verdict = run(&app.ephemeral, &ports, &req, deployed, &mut |url| {
        let scenarios = confirmed_scenarios(app, &req.project);
        if scenarios.is_empty() {
            return Ok("no user-test scenarios to run".to_string());
        }
        crate::user_test::smoke(app, &req.project, url, &commit, &scenarios)?;
        Ok(format!("{} user-test scenario(s) passed", scenarios.len()))
    });
    Ok(verdict)
}

/// The project's confirmed user-test scenarios, each once (the smoke set a deploy runs).
fn confirmed_scenarios(app: &App, project: &str) -> Vec<Scenario> {
    let mut set: Vec<Scenario> = Vec::new();
    for json in app.reviews.confirmed_scenarios(project) {
        if let Ok(scenario) = serde_json::from_str::<Scenario>(&json)
            && scenario.check().is_ok()
            && !set.iter().any(|s| s.id == scenario.id)
        {
            set.push(scenario);
        }
    }
    set
}

/// The last [`TAIL`] characters of a failing command's output.
fn tail(text: &str) -> String {
    let count = text.chars().count();
    if count <= TAIL {
        return text.to_string();
    }
    text.chars().skip(count - TAIL).collect()
}
