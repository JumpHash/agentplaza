//! The driver (PLAN 21.1): a workflow's run walked through the recipe engine. Each ready step runs on its own thread (the
//! steps of a job run one after another, the jobs in parallel); each result is recorded on the run as a check (its
//! commands and its log) and settled through the walker, which starts what it opens. An `approval` step waits for the
//! owner: it is left running, and the owner's decision settles it and walks on (`mod.rs`).
//!
//! The effects of a step go through [`Plant`]: the production one (`live.rs`) runs commands in the job's fork, deploys
//! through the deploy path and asks the owner through Approvals; the tests give a fake. Ephemeral instances are kept here
//! per run, so a later step (`against`, `teardown`) and the end of the run find them, and the run's end takes down the
//! ones it did not keep.

use std::collections::{BTreeMap, VecDeque};
use std::sync::mpsc;

use crate::base_recipe::{Command, WORKSPACE};
use crate::base_vm::Answer;
use crate::recipes::graph::{Move, Report};
use crate::recipes::model::Recipe;
use crate::recipes::runs::{CheckRow, RunState, Runs};

use super::envs::{Envs, Instance, Subject, UpRequest};
use super::model::{StepKind, Work};

/// What a run is for: its project, the change it lands (a `land`, `push` or manual run), the release it is for (a
/// `release.candidate` run), read from the run's subject (`change:<id>` or `release:<version>`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunInfo {
    pub project: String,
    pub change: Option<String>,
    pub release: Option<String>,
}

impl RunInfo {
    /// The info of a run's record.
    pub fn of(project: &str, subject: Option<&str>) -> RunInfo {
        let mut info = RunInfo {
            project: project.to_string(),
            ..RunInfo::default()
        };
        match subject.and_then(|s| s.split_once(':')) {
            Some(("change", id)) if !id.is_empty() => info.change = Some(id.to_string()),
            Some(("release", v)) if !v.is_empty() => info.release = Some(v.to_string()),
            _ => {}
        }
        info
    }

    /// The subject a run records for this info.
    pub fn subject(&self) -> Option<String> {
        self.change
            .as_ref()
            .map(|c| format!("change:{c}"))
            .or_else(|| self.release.as_ref().map(|r| format!("release:{r}")))
    }
}

/// The effects a run's steps make. A job's commands run in its fork; the rest is the hub's.
pub trait Plant: Sync {
    /// What the run is for.
    fn info(&self) -> RunInfo;
    /// Runs one command in the fork of the job `work` belongs to (started on first use, on the pool `work.runs_on`).
    fn exec(&self, work: &Work, command: &Command) -> Result<Answer, String>;
    /// The commands of a `test` step: the change's selection, or the full suite.
    fn tests(&self, select: &str) -> Result<Vec<Command>, String>;
    /// The commands of a `build` step: the project's build commands.
    fn build(&self) -> Result<Vec<Command>, String>;
    /// Deploys `version` (or the environment's default) to a named environment: the summary, or why it failed.
    fn deploy(&self, environment: &str, version: Option<&str>) -> Result<String, String>;
    /// The value of a vault secret, for an env value (`${{ secrets.NAME }}`).
    fn secret(&self, name: &str) -> Result<String, String>;
    /// Asks the owner about an `approval` step: a decision comes back through Approvals.
    fn approval(&self, run: &str, step: &str, message: &str) -> Result<(), String>;
    /// The ephemeral environments.
    fn envs(&self) -> &dyn Envs;
    /// The end of a session: the forks are retired.
    fn retire(&self);
}

/// An ephemeral instance a run holds: whether the run keeps it past its end, and whether it was taken down.
#[derive(Debug, Clone)]
pub struct Held {
    pub instance: Instance,
    pub keep: bool,
    pub down: bool,
}

/// The instances of each run, kept for the run's life in the process (a restart does not forget their TTL: the
/// ephemeral environments expire on their own).
static HELD: std::sync::Mutex<BTreeMap<String, Vec<Held>>> = std::sync::Mutex::new(BTreeMap::new());

fn held() -> std::sync::MutexGuard<'static, BTreeMap<String, Vec<Held>>> {
    HELD.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Keeps an instance a run brought up.
pub fn hold(run: &str, instance: Instance, keep: bool) {
    held().entry(run.to_string()).or_default().push(Held {
        instance,
        keep,
        down: false,
    });
}

/// The latest instance of a run that is still up.
pub fn live_instance(run: &str) -> Option<Instance> {
    held()
        .get(run)?
        .iter()
        .rev()
        .find(|h| !h.down)
        .map(|h| h.instance.clone())
}

/// Forgets every run's instances: the tests' start (a store numbers its runs from one, so two tests' runs share ids).
#[cfg(test)]
pub fn forget_all() {
    held().clear();
}

/// Walks the run from `ready` until nothing is left to start. Returns when every started step has settled.
pub fn walk(runs: &Runs, plant: &dyn Plant, run: &str, ready: Vec<String>) -> Result<(), String> {
    let view = runs.view(run).ok_or_else(|| format!("no run {run}"))?;
    let works = works_of(&view.recipe);
    let (tx, rx) = mpsc::channel::<(String, Report, CheckRow)>();
    let mut queue: VecDeque<String> = ready.into();
    let mut inflight = 0usize;
    std::thread::scope(|scope| {
        loop {
            while let Some(step) = queue.pop_front() {
                let Some(work) = works.get(&step) else {
                    continue;
                };
                let now = apz_plaza::now_ms();
                runs.started(run, &step, now);
                if let StepKind::Approval { message } = &work.step {
                    if let Err(why) = plant.approval(run, &step, message) {
                        let row = CheckRow {
                            step: step.clone(),
                            passed: false,
                            commands: "approval".into(),
                            summary: why,
                        };
                        runs.record_check(run, row, now);
                        for m in runs.settle(run, &step, &Report::verdict(false), now) {
                            if let Move::Ready(next) = m {
                                queue.push_back(next);
                            }
                        }
                    }
                    continue;
                }
                inflight += 1;
                let tx = tx.clone();
                let tests = matches!(work.step, StepKind::Test { .. });
                scope.spawn(move || {
                    let (report, row) = execute(plant, run, &step, work, tests);
                    let _ = tx.send((step, report, row));
                });
            }
            if inflight == 0 {
                break;
            }
            let Ok((step, report, row)) = rx.recv() else {
                break;
            };
            inflight -= 1;
            let now = apz_plaza::now_ms();
            runs.record_check(run, row, now);
            for m in runs.settle(run, &step, &report, now) {
                if let Move::Ready(next) = m {
                    queue.push_back(next);
                }
            }
        }
    });
    Ok(())
}

/// What a step did, shown on its check: the commands it ran (as the file wrote them, secrets never shown) and its log.
struct Shown {
    commands: String,
    summary: String,
}

/// Runs one step and reads its result as the report the walk settles with, and the check it records.
fn execute(
    plant: &dyn Plant,
    run: &str,
    step: &str,
    work: &Work,
    tests: bool,
) -> (Report, CheckRow) {
    let (passed, shown) = match perform(plant, run, work) {
        Ok(shown) => (true, shown),
        Err(shown) => (false, shown),
    };
    let report = Report {
        passed,
        tests: tests.then_some(passed),
        ..Report::default()
    };
    let row = CheckRow {
        step: step.to_string(),
        passed,
        commands: shown.commands,
        summary: shown.summary,
    };
    (report, row)
}

fn perform(plant: &dyn Plant, run: &str, work: &Work) -> Result<Shown, Shown> {
    let fail = |why: &str| Shown {
        commands: work.step.name().into(),
        summary: why.to_string(),
    };
    match &work.step {
        StepKind::Run { run: line, env, .. } => {
            let text = expand(plant, run, work, line, false).map_err(|e| fail(&e))?;
            let (argv, secrets) = argv_for(plant, run, work, env, &text).map_err(|e| fail(&e))?;
            let command = command_of("run", argv, work.step.timeout_s());
            let answer = plant.exec(work, &command).map_err(|e| fail(&e))?;
            let log = redact(&tail(&answer.output), &secrets);
            let summary = if answer.ok {
                log
            } else {
                format!("{}: {log}", exit_of(&answer))
            };
            let shown = Shown {
                commands: line.clone(),
                summary,
            };
            if answer.ok { Ok(shown) } else { Err(shown) }
        }
        StepKind::Test { select } => {
            let commands = plant.tests(select).map_err(|e| fail(&e))?;
            run_all(plant, work, "test", commands)
        }
        StepKind::Build {} => {
            let commands = plant.build().map_err(|e| fail(&e))?;
            run_all(plant, work, "build", commands)
        }
        StepKind::Deploy {
            environment,
            ephemeral,
            ttl_minutes,
            keep,
            version,
        } => {
            if *ephemeral {
                let info = plant.info();
                let subject = match (info.release, info.change) {
                    (Some(release), _) => Subject::Release(release),
                    (None, Some(change)) => Subject::Lane(change),
                    (None, None) => {
                        return Err(fail("an ephemeral deploy needs a change or a release"));
                    }
                };
                let req = UpRequest {
                    project: info.project,
                    environment: environment.clone(),
                    subject,
                    ttl_minutes: *ttl_minutes,
                };
                let instance = plant.envs().up(&req).map_err(|e| fail(&e))?;
                let summary = match &instance.url {
                    Some(url) => format!("instance {} is up at {url}", instance.id),
                    None => format!("instance {} is up (no address yet)", instance.id),
                };
                hold(run, instance, *keep);
                Ok(Shown {
                    commands: format!("deploy {environment} (ephemeral)"),
                    summary,
                })
            } else {
                let version = match version {
                    Some(v) => Some(expand(plant, run, work, v, false).map_err(|e| fail(&e))?),
                    None => None,
                };
                let summary = plant
                    .deploy(environment, version.as_deref())
                    .map_err(|e| fail(&e))?;
                Ok(Shown {
                    commands: format!("deploy {environment}"),
                    summary,
                })
            }
        }
        StepKind::Promote {
            environment,
            version,
        } => {
            let version = match version {
                Some(v) => Some(expand(plant, run, work, v, false).map_err(|e| fail(&e))?),
                None => plant.info().release,
            };
            if version.is_none() {
                return Err(fail("a promote names a version, or runs for a release"));
            }
            let summary = plant
                .deploy(environment, version.as_deref())
                .map_err(|e| fail(&e))?;
            Ok(Shown {
                commands: format!("promote {environment}"),
                summary,
            })
        }
        StepKind::Against { run: line, env, .. } => {
            let instance = live_instance(run)
                .ok_or_else(|| fail("no ephemeral instance is up to test against"))?;
            let text = expand(plant, run, work, line, false).map_err(|e| fail(&e))?;
            let mut exports = String::new();
            for (name, value) in env {
                let value = expand(plant, run, work, value, true).map_err(|e| fail(&e))?;
                exports.push_str(&format!("export {name}={}; ", shell_quote(&value)));
            }
            let command = format!("{exports}{text}");
            let shown = Shown {
                commands: line.clone(),
                summary: String::new(),
            };
            match plant.envs().against(&instance, &command) {
                Ok(output) => Ok(Shown {
                    summary: tail(&output),
                    ..shown
                }),
                Err(why) => Err(Shown {
                    summary: tail(&why),
                    ..shown
                }),
            }
        }
        StepKind::Teardown {} => {
            let mut done = Vec::new();
            let mut failed = Vec::new();
            for result in take_down(run, plant.envs(), true) {
                match result {
                    Ok(id) => done.push(id),
                    Err(why) => failed.push(why),
                }
            }
            let summary = if failed.is_empty() {
                format!("took down {}", list_or_none(&done))
            } else {
                format!("took down {}; {}", list_or_none(&done), failed.join("; "))
            };
            if failed.is_empty() {
                Ok(Shown {
                    commands: "teardown".into(),
                    summary,
                })
            } else {
                Err(Shown {
                    commands: "teardown".into(),
                    summary,
                })
            }
        }
        // An approval never reaches here: the walk leaves it to the owner.
        StepKind::Approval { message } => Ok(Shown {
            commands: "approval".into(),
            summary: message.clone(),
        }),
    }
}

/// Runs each command of a job's check in its fork, in order; the first that fails fails the step.
fn run_all(
    plant: &dyn Plant,
    work: &Work,
    label: &str,
    commands: Vec<Command>,
) -> Result<Shown, Shown> {
    let names: Vec<String> = commands.iter().map(|c| c.argv.join(" ")).collect();
    let shown_commands = names.join(" && ");
    if commands.is_empty() {
        return Ok(Shown {
            commands: label.into(),
            summary: "nothing to run".into(),
        });
    }
    let mut log = String::new();
    for command in &commands {
        let answer = plant.exec(work, command).map_err(|why| Shown {
            commands: shown_commands.clone(),
            summary: format!("{}: {why}", command.label),
        })?;
        log.push_str(&format!(
            "$ {}\n{}",
            command.argv.join(" "),
            tail(&answer.output)
        ));
        log.push('\n');
        if !answer.ok {
            return Err(Shown {
                commands: shown_commands,
                summary: format!("{} {}: {}", command.label, exit_of(&answer), tail(&log)),
            });
        }
    }
    Ok(Shown {
        commands: shown_commands,
        summary: tail(&log),
    })
}

/// Takes down the run's instances that are up and not kept (or all of them, for an explicit teardown). Each result is the
/// instance's id, or why it could not come down.
pub fn take_down(run: &str, envs: &dyn Envs, all: bool) -> Vec<Result<String, String>> {
    let mut out = Vec::new();
    let mut guard = held();
    let Some(list) = guard.get_mut(run) else {
        return out;
    };
    for h in list.iter_mut().filter(|h| !h.down && (all || !h.keep)) {
        match envs.down(&h.instance) {
            Ok(()) => {
                h.down = true;
                out.push(Ok(h.instance.id.clone()));
            }
            Err(why) => out.push(Err(format!("{}: {why}", h.instance.id))),
        }
    }
    out
}

/// The end of a run: the instances it did not keep come down, whatever the run's outcome. Returns the teardown's check,
/// when there was anything to take down.
pub fn finish(runs: &Runs, envs: &dyn Envs, run: &str, now: u64) {
    let ended = runs
        .view(run)
        .is_some_and(|v| v.run.state != RunState::Running && v.run.state != RunState::Owner);
    if !ended {
        return;
    }
    let results = take_down(run, envs, false);
    if results.is_empty() {
        return;
    }
    let ids: Vec<String> = results
        .iter()
        .map(|r| match r {
            Ok(id) => id.clone(),
            Err(why) => why.clone(),
        })
        .collect();
    let passed = results.iter().all(Result::is_ok);
    runs.record_check(
        run,
        CheckRow {
            step: "teardown".into(),
            passed,
            commands: "teardown at the end of the run".into(),
            summary: format!("took down {}", ids.join(", ")),
        },
        now,
    );
}

/// The `${{ ... }}` expressions of a text filled in: a matrix value, the run's ephemeral URL, its release or change, and (in
/// an env value only) a vault secret.
fn expand(
    plant: &dyn Plant,
    run: &str,
    work: &Work,
    text: &str,
    secrets: bool,
) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 3..];
        let end = after
            .find("}}")
            .ok_or_else(|| format!("`{text}` has an expression that does not close"))?;
        let expr = after[..end].trim();
        let value = match expr {
            "env.url" => live_instance(run)
                .and_then(|i| i.url)
                .ok_or("no ephemeral environment has an address yet: a deploy with ephemeral = true comes first")?,
            "release.version" => plant
                .info()
                .release
                .ok_or("this run is not for a release")?,
            "change" => plant.info().change.ok_or("this run is not for a change")?,
            _ => {
                if let Some(key) = expr.strip_prefix("matrix.") {
                    work.matrix
                        .get(key)
                        .cloned()
                        .ok_or_else(|| format!("${{{{ {expr} }}}} names no matrix key"))?
                } else if let Some(name) = expr.strip_prefix("secrets.") {
                    if !secrets {
                        return Err(format!("${{{{ {expr} }}}}: a secret is read in an env value"));
                    }
                    plant.secret(name)?
                } else {
                    return Err(format!("${{{{ {expr} }}}} is not an expression"));
                }
            }
        };
        out.push_str(&value);
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The argument vector of a shell line with its env: `sh -c line`, behind `env K=V` when there are values (never a
/// shell, so the values are not parsed). Also the secret values it resolved, which a log never shows.
fn argv_for(
    plant: &dyn Plant,
    run: &str,
    work: &Work,
    env: &BTreeMap<String, String>,
    line: &str,
) -> Result<(Vec<String>, Vec<String>), String> {
    let mut argv = Vec::new();
    let mut secrets = Vec::new();
    if !env.is_empty() {
        argv.push("env".to_string());
        for (name, raw) in env {
            let value = expand(plant, run, work, raw, true)?;
            if raw.contains("secrets.") {
                secrets.push(value.clone());
            }
            argv.push(format!("{name}={value}"));
        }
    }
    argv.extend(["sh".to_string(), "-c".to_string(), line.to_string()]);
    Ok((argv, secrets))
}

/// The text with each secret value replaced: a log never shows what the vault holds.
fn redact(text: &str, secrets: &[String]) -> String {
    secrets
        .iter()
        .filter(|s| !s.is_empty())
        .fold(text.to_string(), |text, secret| {
            text.replace(secret.as_str(), "[secret]")
        })
}

fn command_of(label: &str, argv: Vec<String>, timeout_s: u64) -> Command {
    Command {
        label: label.to_string(),
        argv,
        cwd: WORKSPACE,
        timeout_s,
        env: Vec::new(),
        listen: None,
    }
}

fn exit_of(answer: &Answer) -> String {
    match answer.exit_code {
        Some(code) => format!("exit {code}"),
        None => "no exit status".to_string(),
    }
}

fn list_or_none(ids: &[String]) -> String {
    if ids.is_empty() {
        "nothing".into()
    } else {
        ids.join(", ")
    }
}

/// Quotes a value for a POSIX shell.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// The last 1200 characters of `text`: where a failing command says what went wrong.
pub fn tail(text: &str) -> String {
    let trimmed = text.trim_end();
    let count = trimmed.chars().count();
    if count <= crate::checks::TAIL_CHARS {
        return trimmed.to_string();
    }
    trimmed
        .chars()
        .skip(count - crate::checks::TAIL_CHARS)
        .collect()
}

/// The works of a recipe's steps, by step id.
pub fn works_of(recipe: &Recipe) -> BTreeMap<String, Work> {
    recipe
        .steps
        .iter()
        .filter_map(|s| s.work.clone().map(|w| (s.id.clone(), w)))
        .collect()
}
