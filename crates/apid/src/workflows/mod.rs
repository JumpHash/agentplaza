//! Workflows (PLAN 21, 23): a project's CI/CD pipelines, declared in its repo at `.plaza/workflows/<name>.toml`
//! ([`model`]) and run by the recipe engine ([`drive`], with the production effects in [`live`]). Ephemeral environments
//! go through [`envs`]. A project's GitHub Actions workflows are converted to plaza's format by [`github`].
//!
//! A run starts from a trigger: a change landed (`land`), a change's revision sealed (`push`), a release proposed
//! (`release.candidate`), a cron schedule (its trigger row, fired by the scheduler), or the owner's hand (`manual`). The
//! run's record is the recipe engine's (`recipe_run`, with the recipe `workflow:<name>` it ran as a snapshot), so the run
//! view shows it like any other run. An `approval` step is an item in Approvals (kind `workflow`): its decision comes back
//! through [`decided`].

pub mod drive;
pub mod envs;
pub mod github;
pub mod live;
pub mod model;
pub mod routes;

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::sync::Arc;

use apz_blobs::Hash;
use serde_json::{Value, json};

use crate::approvals::Approval;
use crate::github_modes::{self, RunPhase};
use crate::recipes::graph::{Move, Report};
use crate::recipes::runs::{CheckRow, RunState, Runs};
use crate::state::App;
use crate::triggers::{Action, TriggerRow, Triggers};

use drive::RunInfo;
use model::{DIR, Known, Workflow};

/// The recipe id prefix of a workflow (`workflow:<name>`), which a schedule's trigger row names too.
pub const PREFIX: &str = "workflow:";

/// A workflow file of a project that parsed.
#[derive(Debug, Clone)]
pub struct Listed {
    pub file: String,
    pub workflow: Workflow,
}

/// The revision of a project's repo at main.
pub fn main_rev(app: &App, project: &str) -> Result<String, String> {
    let repo = repo_of(app, project)?;
    let detail = app.weft.get(&format!("/repos/{repo}"))?;
    detail["refs"]["main"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("{repo} has no main"))
}

fn repo_of(app: &App, project: &str) -> Result<String, String> {
    app.projects
        .get(project)
        .and_then(|p| p.repo)
        .ok_or_else(|| format!("{project} has no repo yet"))
}

/// The files of `dir` on main (directly in it) whose names end in one of `suffixes`: each path with its text, or why it
/// could not be read.
pub fn files_on_main(
    app: &App,
    project: &str,
    dir: &str,
    suffixes: &[&str],
) -> Result<Vec<(String, Result<String, String>)>, String> {
    let rev = main_rev(app, project)?;
    let listing = app.weft.get(&format!("/revisions/{rev}/files"))?;
    let client = app.weft.client("hub")?;
    let mut out: Vec<(String, Result<String, String>)> = Vec::new();
    for file in listing["files"].as_array().into_iter().flatten() {
        let Some(path) = file["path"].as_str() else {
            continue;
        };
        let direct = path
            .strip_prefix(dir)
            .and_then(|rest| rest.strip_prefix('/'))
            .is_some_and(|rest| !rest.contains('/'));
        if !direct || !suffixes.iter().any(|s| path.ends_with(s)) {
            continue;
        }
        let text = client
            .get_bytes(&format!("/revisions/{rev}/file?path={path}"))
            .and_then(|bytes| String::from_utf8(bytes).map_err(|_| format!("{path} is not UTF-8")));
        out.push((path.to_string(), text));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// The text of one file on main.
fn file_on_main(app: &App, project: &str, path: &str) -> Result<String, String> {
    let rev = main_rev(app, project)?;
    let client = app.weft.client("hub")?;
    let bytes = client.get_bytes(&format!("/revisions/{rev}/file?path={path}"))?;
    String::from_utf8(bytes).map_err(|_| format!("{path} is not UTF-8"))
}

/// What the checks of a workflow read from the project: its environments and its pools.
pub fn known(app: &App, project: &str) -> Known {
    let environments = app
        .environments
        .of(project)
        .into_iter()
        .map(|e| e.name)
        .collect();
    let pools = match crate::project_stack::read(app, project) {
        Ok(Some(stack)) => Some(crate::base_builds::pool_keys(&stack)),
        _ => None,
    };
    Known {
        environments: Some(environments),
        pools,
    }
}

/// A workflow's text, parsed and checked as the file at `file` (its name is the file's stem).
pub fn parse_as(file: &str, text: &str, known: &Known) -> Result<Workflow, String> {
    let stem = file
        .rsplit('/')
        .next()
        .unwrap_or(file)
        .trim_end_matches(".toml");
    let workflow = Workflow::parse(stem, text).map_err(|e| format!("{file}: {e}"))?;
    if workflow.name != stem {
        return Err(format!(
            "{file} names the workflow {}: the file's name is the workflow's name",
            workflow.name
        ));
    }
    workflow.check(known).map_err(|e| format!("{file}: {e}"))?;
    Ok(workflow)
}

/// The project's workflows on main: those that parse, and the files that do not (with why).
pub fn list(app: &App, project: &str) -> Result<(Vec<Listed>, Vec<(String, String)>), String> {
    let known = known(app, project);
    let mut workflows = Vec::new();
    let mut errors = Vec::new();
    for (file, text) in files_on_main(app, project, DIR, &[".toml"])? {
        match text.and_then(|t| parse_as(&file, &t, &known)) {
            Ok(workflow) => workflows.push(Listed { file, workflow }),
            Err(why) => errors.push((file, why)),
        }
    }
    Ok((workflows, errors))
}

/// One workflow of a project, from main.
pub fn load(app: &App, project: &str, name: &str) -> Result<Listed, String> {
    if !model::valid_name(name) {
        return Err(format!("{name} is not a workflow name"));
    }
    let file = format!("{DIR}/{name}.toml");
    let text = file_on_main(app, project, &file)?;
    let workflow = parse_as(&file, &text, &known(app, project))?;
    Ok(Listed { file, workflow })
}

/// The GitHub Actions workflows of a project on main, converted: each with its report, and the files that did not convert.
pub fn imports(
    app: &App,
    project: &str,
) -> Result<(Vec<github::Imported>, Vec<(String, String)>), String> {
    let mut converted = Vec::new();
    let mut errors = Vec::new();
    for (file, text) in files_on_main(app, project, ".github/workflows", &[".yml", ".yaml"])? {
        match text.and_then(|t| github::convert(&file, &t)) {
            Ok(imported) => converted.push(imported),
            Err(why) => errors.push((file, why)),
        }
    }
    Ok((converted, errors))
}

/// Starts a run of workflow `name` of `project` on `trigger`, for what `info` names. Its steps run on a driver thread.
pub fn start(
    app: &Arc<App>,
    project: &str,
    name: &str,
    trigger: &str,
    info: &RunInfo,
) -> Result<String, String> {
    let listed = load(app, project, name)?;
    let recipe = listed.workflow.compile()?;
    let subject = info.subject();
    let (run, moves) = app.recipe_runs.start(
        &recipe,
        "workflow",
        trigger,
        None,
        Some(project),
        subject.as_deref(),
        apz_plaza::now_ms(),
    );
    report_started(app, &run);
    spawn_run(app, run.clone(), ready_of(moves));
    Ok(run)
}

/// The GitHub side of a run's start and end (4.7): in `ci` or `mirror` mode the run is a check run on its commit. A
/// failure to reach GitHub is printed; it does not fail the run.
fn report_started(app: &App, run: &str) {
    report(app, run, None);
}

/// The end of a run, reported as [`report_started`] reports its start.
pub fn report_finished(app: &App, run: &str) {
    report(app, run, Some(()));
}

fn report(app: &App, run: &str, finished: Option<()>) {
    let Some(view) = app.recipe_runs.view(run) else {
        return;
    };
    let Some(project) = view.run.project.clone() else {
        return;
    };
    let workflow = view
        .run
        .recipe
        .strip_prefix(PREFIX)
        .unwrap_or(&view.run.recipe);
    let outcome = view.run.outcome.clone().unwrap_or_default();
    let phase = match finished {
        None => RunPhase::Started,
        Some(()) => RunPhase::Finished {
            passed: view.run.state == RunState::Passed,
            summary: &outcome,
        },
    };
    if let Err(why) = github_modes::report_run(
        app,
        &project,
        view.run.subject.as_deref(),
        run,
        workflow,
        phase,
    ) {
        eprintln!("apid: workflow {workflow} of {project} on GitHub: {why}");
    }
}

/// The workflow runs of one change, newest first (the lane's view of them, as the CI runs are its view).
pub fn runs_of_change(app: &App, project: &str, change: &str) -> Vec<Value> {
    let subject = format!("change:{change}");
    app.recipe_runs
        .list(None, Some(project))
        .iter()
        .filter(|v| {
            v.run.recipe.starts_with(PREFIX) && v.run.subject.as_deref() == Some(subject.as_str())
        })
        .map(crate::recipes::routes::summary)
        .collect()
}

fn ready_of(moves: Vec<Move>) -> Vec<String> {
    moves
        .into_iter()
        .filter_map(|m| match m {
            Move::Ready(step) => Some(step),
            _ => None,
        })
        .collect()
}

fn spawn_run(app: &Arc<App>, run: String, ready: Vec<String>) {
    let app = Arc::clone(app);
    std::thread::spawn(move || live::run_live(&app, &run, ready));
}

/// Whether a run of this workflow for this trigger and subject already exists (`active_only`: one still going).
fn already_started(
    app: &App,
    project: &str,
    name: &str,
    trigger: &str,
    subject: &str,
    active_only: bool,
) -> bool {
    let recipe = format!("{PREFIX}{name}");
    app.recipe_runs.list(None, Some(project)).iter().any(|v| {
        v.run.recipe == recipe
            && v.run.trigger == trigger
            && v.run.subject.as_deref() == Some(subject)
            && (!active_only || matches!(v.run.state, RunState::Running | RunState::Owner))
    })
}

/// A Weft event the workflows start runs for: a change landed, a revision sealed (a push to its lane), a release proposed.
/// The runs start on a thread, since reading the repo is Weft's work.
pub fn on_weft(app: &App, kind: &str, body: &Value) {
    let trigger = match kind {
        "land" => "land",
        "revision.seal" => "push",
        "release.propose" => "release.candidate",
        _ => return,
    };
    let repo = body["repo"].as_str().unwrap_or_default();
    let Some(project) = crate::github_modes::project_of(app, repo).map(|p| p.id) else {
        return;
    };
    let subject = match trigger {
        "release.candidate" => body["version"].as_str().map(|v| format!("release:{v}")),
        _ => body["change"].as_str().map(|c| format!("change:{c}")),
    };
    let (Some(subject), Some(app)) = (subject, app.handle().upgrade()) else {
        return;
    };
    std::thread::spawn(move || {
        let (listed, errors) = match list(&app, &project) {
            Ok(found) => found,
            Err(why) => {
                eprintln!("apid: workflows of {project}: {why}");
                return;
            }
        };
        for (file, why) in errors {
            eprintln!("apid: workflow file {file} of {project} starts no run: {why}");
        }
        sync_schedules(&app, &project, &listed);
        for item in listed
            .iter()
            .filter(|l| l.workflow.runs_on_trigger(trigger))
        {
            let name = item.workflow.name.as_str();
            if already_started(&app, &project, name, trigger, &subject, trigger == "push") {
                continue;
            }
            let info = RunInfo::of(&project, Some(&subject));
            if let Err(why) = start(&app, &project, name, trigger, &info) {
                eprintln!("apid: workflow {name} of {project}: {why}");
            }
        }
    });
}

/// The id of a workflow's schedule row: one row per workflow, named for its project.
fn schedule_id(project: &str, name: &str) -> String {
    format!("workflow-schedule:{project}:{name}")
}

/// Whether workflow `name` is on main and declares a schedule: its schedule row, and the fire of it, are current.
fn scheduled_in(listed: &[Listed], name: &str) -> bool {
    listed
        .iter()
        .any(|l| l.workflow.name == name && l.workflow.runs_on_trigger("schedule"))
}

/// Keeps the schedule rows of the workflows that run on a cron: one row per workflow, with the cron its file says. The
/// rows of the project's workflows that no longer declare a schedule, or no longer exist, are removed.
pub fn sync_schedules(app: &App, project: &str, listed: &[Listed]) {
    sync_rows(
        &app.triggers,
        &app.recipe_runs,
        project,
        listed,
        apz_plaza::now_ms(),
    );
}

/// [`sync_schedules`] over the given stores. The workflows named are those on main and those the project has run: a
/// deleted workflow that ran still has its row, and one that never ran is dropped when its row would fire.
fn sync_rows(triggers: &Triggers, runs: &Runs, project: &str, listed: &[Listed], now: u64) {
    let mut names: BTreeSet<String> = listed.iter().map(|l| l.workflow.name.clone()).collect();
    names.extend(
        runs.list(None, Some(project))
            .iter()
            .filter_map(|v| v.run.recipe.strip_prefix(PREFIX).map(str::to_string)),
    );
    for name in &names {
        if !scheduled_in(listed, name) {
            triggers.remove(&schedule_id(project, name));
        }
    }
    for item in listed
        .iter()
        .filter(|l| l.workflow.runs_on_trigger("schedule"))
    {
        let id = schedule_id(project, &item.workflow.name);
        if triggers
            .get(&id)
            .is_some_and(|existing| existing.cron == item.workflow.cron)
        {
            continue;
        }
        let mut row = TriggerRow {
            id: id.clone(),
            recipe: format!("{PREFIX}{}", item.workflow.name),
            project: Some(project.to_string()),
            action: Action::Run,
            source: "schedule".into(),
            event: String::new(),
            label: String::new(),
            cron: item.workflow.cron.clone(),
            every_minutes: 0,
            window_ms: 0,
            enabled: true,
            next_ms: None,
            last_ms: None,
            created_ms: now,
        };
        row.next_ms = row.next_after(now);
        triggers.remove(&id);
        triggers.ensure(row);
    }
}

/// Whether a schedule row's fire starts its run: its workflow is on main and still declares the schedule. A row that
/// outlived either is removed, and its fire starts nothing.
fn fire_is_current(triggers: &Triggers, row: &str, name: &str, listed: &[Listed]) -> bool {
    if scheduled_in(listed, name) {
        return true;
    }
    triggers.remove(row);
    false
}

/// A scheduled run of a workflow: the scheduler's fire of its trigger row. The row is current only while main still
/// declares its schedule; a fire that is not current starts nothing.
pub fn fire_schedule(app: &Arc<App>, row: &TriggerRow) {
    let (Some(name), Some(project)) = (row.recipe.strip_prefix(PREFIX), row.project.as_deref())
    else {
        return;
    };
    let listed = match list(app, project) {
        Ok((listed, _)) => listed,
        Err(why) => {
            eprintln!("apid: scheduled workflow {name} of {project} starts no run: {why}");
            return;
        }
    };
    if !fire_is_current(&app.triggers, &row.id, name, &listed) {
        return;
    }
    let info = RunInfo::of(project, None);
    if let Err(why) = start(app, project, name, "schedule", &info) {
        eprintln!("apid: scheduled workflow {name} of {project}: {why}");
    }
}

/// The owner's decision on a `workflow` approval: the approval step settles, and the run walks on (or fails).
pub fn decided(app: &App, item: &Approval, approve: bool) -> Result<(), String> {
    let run = item.detail["run"]
        .as_str()
        .ok_or("the approval names no run")?;
    let step = item.detail["step"]
        .as_str()
        .ok_or("the approval names no step")?;
    let now = apz_plaza::now_ms();
    app.recipe_runs.record_check(
        run,
        CheckRow {
            step: step.to_string(),
            passed: approve,
            commands: "approval".into(),
            summary: if approve {
                "approved by the owner".into()
            } else {
                "refused by the owner".into()
            },
        },
        now,
    );
    let moves = app.recipe_runs.settle(
        run,
        step,
        &Report {
            passed: approve,
            owner: Some(approve),
            ..Report::default()
        },
        now,
    );
    if let Some(arc) = app.handle().upgrade() {
        spawn_run(&arc, run.to_string(), ready_of(moves));
    }
    Ok(())
}

/// Saves a file as a change of the project's repo, for the owner to land: the file's bytes are uploaded, a lane is opened,
/// and a revision with the file is sealed (the calls project memory makes). Returns the change's id.
pub fn save_change(
    app: &App,
    project: &str,
    path: &str,
    text: &str,
    title: &str,
) -> Result<String, String> {
    let repo = repo_of(app, project)?;
    let bytes = text.as_bytes().to_vec();
    let hashes: Vec<Hash> = vec![Hash::of(&bytes)];
    let bytes_of = |hash: &Hash| {
        if *hash == hashes[0] {
            Ok(bytes.clone())
        } else {
            Err("no bytes for the file's hash".to_string())
        }
    };
    app.weft.client("hub")?.upload(&hashes, &bytes_of)?;
    let opened = app.weft.post(
        "/lanes",
        &json!({ "repo": repo, "title": title, "author": { "kind": "hub" } }),
    )?;
    let change = opened["change"]
        .as_str()
        .ok_or("Weft opened no lane for the workflow change")?
        .to_string();
    let deltas = json!([{ "path": path, "after": hashes[0], "mode": 0o100644 }]);
    app.weft.hub(
        "revision.draft",
        json!({ "change": change, "n": 1, "deltas": deltas, "tool_calls": ["workflows"] }),
    )?;
    app.weft.hub(
        "revision.seal",
        json!({ "change": change, "n": 1, "deltas": deltas }),
    )?;
    app.weft.hub(
        "intent.set",
        json!({ "change": change, "n": 1, "intents": [{ "title": title, "kind": "change", "covers": [path] }] }),
    )?;
    Ok(change)
}
