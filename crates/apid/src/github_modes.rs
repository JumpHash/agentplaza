//! GitHub's two modes (PLAN 21.5, 18.4). A project's `github` setting (`none`, `mirror`, `ci`) says which one its repo
//! runs in.
//!
//! - `mirror`: Weft is the truth. Each landing is exported to the connection's branch (weft_hub::export_all: one commit
//!   per intent, fast-forward, signed). Nothing here.
//! - `ci`: GitHub is the truth. The queue's head lane is pushed as a branch (`agentplaza/<change>`), opened as a pull
//!   request into the repo's main, and gated by its check runs: GitHub's own checks are the gate, recorded as a run of
//!   the `ci` recipe like the plaza's checks (checks.rs), with the same rows. When GitHub merges the pull request, Weft
//!   lands the lane through its queue and records GitHub's merge commit as the connection's head. When GitHub's main
//!   moves on its own (a merge that was not a lane), what landed there is imported (import.rs) first.
//!
//! weft_hub::land_queues calls [`ci_step`] for a `ci` repo's queue instead of Weft's own step. Everything reaches GitHub
//! through the repo's connection's authorization (platforms::access) and the platform's API (platforms::github).

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde_json::{Value, json};

use crate::platforms::github::{CheckRun, CheckWrite, GitHub, Pull};
use crate::platforms::model::Kind;
use crate::recipes::runs::{CheckRow, source_of};
use crate::state::App;

/// The mode of a project that runs in `ci`.
pub const CI: &str = "ci";
/// The trigger a GitHub gate's run records.
pub const TRIGGER: &str = "github";
/// The check conclusions that pass the gate (GitHub's own words).
const PASSING: &[&str] = &["success", "neutral", "skipped"];

/// The project of `repo` (its Weft repo), when a project has it.
pub fn project_of(app: &App, repo: &str) -> Option<crate::projects::Project> {
    app.projects
        .list()
        .into_iter()
        .find(|p| p.repo.as_deref() == Some(repo))
}

/// A repo's GitHub mode: its project's `github` setting, `none` when no project has the repo.
pub fn mode_of(app: &App, repo: &str) -> String {
    project_of(app, repo)
        .map(|p| p.github)
        .unwrap_or_else(|| "none".into())
}

/// The branch a lane is pushed to on GitHub.
pub fn branch_of(change: &str) -> String {
    format!("agentplaza/{change}")
}

/// Where a repo's GitHub connection points: the connection, the repository, the repo's main branch on GitHub, the
/// remote, a token that reaches it, and the API.
pub struct Target {
    pub connection: String,
    pub owner: String,
    pub name: String,
    pub main: String,
    pub remote: String,
    pub token: String,
    pub api: GitHub,
}

/// The GitHub connection of `repo` (the first one whose authorization is a GitHub one).
pub fn target(app: &App, repo: &str) -> Result<Target, String> {
    let detail = app.weft.get(&format!("/repos/{repo}"))?;
    for c in detail["connections"].as_array().into_iter().flatten() {
        let Some(id) = c["credential"]
            .as_str()
            .and_then(|n| n.strip_prefix("platform:"))
        else {
            continue;
        };
        let Some(integ) = app.platforms.integration(id) else {
            continue;
        };
        if integ.kind != Kind::Github {
            continue;
        }
        let remote = s(&c["remote"]);
        let (owner, name) = slug_of(&remote)?;
        let token = crate::platforms::access(app, &integ)?.token;
        return Ok(Target {
            connection: s(&c["id"]),
            owner,
            name,
            main: s(&c["branch"]),
            remote,
            token,
            api: app.platforms.github(&integ.host),
        });
    }
    Err(format!("{repo} has no GitHub connection"))
}

/// `owner` and `name` of a GitHub clone URL (`https://host/owner/name.git`).
pub fn slug_of(remote: &str) -> Result<(String, String), String> {
    let path = remote.trim_end_matches('/').trim_end_matches(".git");
    let mut parts = path.rsplit('/');
    let name = parts.next().filter(|n| !n.is_empty());
    let owner = parts.next().filter(|o| !o.is_empty());
    match (owner, name) {
        (Some(o), Some(n)) => Ok((o.to_string(), n.to_string())),
        _ => Err(format!("{remote} is not owner/name on GitHub")),
    }
}

/// A lane's gate on GitHub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// No checks yet, or some still running.
    Pending,
    /// A check failed.
    Failed,
    /// Every check completed and passed.
    Passed,
}

impl Gate {
    pub fn name(self) -> &'static str {
        match self {
            Gate::Pending => "pending",
            Gate::Failed => "failed",
            Gate::Passed => "passed",
        }
    }
}

/// The gate of a commit's check runs: failed as soon as one completed without passing, passed when every one has
/// completed and passed, pending otherwise (and while there are none).
pub fn gate_of(checks: &[CheckRun]) -> Gate {
    let failed = |c: &CheckRun| {
        c.status == "completed"
            && c.conclusion
                .as_deref()
                .is_some_and(|k| !PASSING.contains(&k))
    };
    if checks.iter().any(failed) {
        return Gate::Failed;
    }
    if checks.is_empty()
        || checks
            .iter()
            .any(|c| c.status != "completed" || c.conclusion.is_none())
    {
        return Gate::Pending;
    }
    Gate::Passed
}

/// A check run as a row of the run (what the step found, in a line).
fn row_of(c: &CheckRun) -> CheckRow {
    CheckRow {
        step: c.name.clone(),
        passed: c.status == "completed"
            && c.conclusion
                .as_deref()
                .is_some_and(|k| PASSING.contains(&k)),
        commands: String::new(),
        summary: c.conclusion.clone().unwrap_or_else(|| c.status.clone()),
    }
}

/// The queue's step for a `ci` repo's `reference`: its head lane is pushed, its pull request is open, and its check runs
/// are the gate. A merge on GitHub lands the lane in Weft. Errors are printed and the queue waits.
pub fn ci_step(app: &App, repo: &str, reference: &str) {
    let t = match target(app, repo) {
        Ok(t) => t,
        Err(e) => return eprintln!("apid: {repo}: {e}"),
    };
    let Ok(queue) = app
        .weft
        .get(&format!("/repos/{repo}/queue?ref={reference}"))
    else {
        return;
    };
    let Some(head) = queue["queue"].as_array().and_then(|q| q.first()) else {
        return;
    };
    let Some(change) = head["change"].as_str().map(str::to_string) else {
        return;
    };
    let title = head["title"].as_str().unwrap_or(&change).to_string();
    let branch = branch_of(&change);
    let mut lane = export_lane(app, repo, &change, &t, &branch);
    if lane.as_ref().is_ok_and(|l| l["state"] == "moved") {
        // GitHub's main moved on its own: what landed there comes in before the lane is pushed again.
        if let Err(e) = sync_main(app, repo, &t) {
            return eprintln!("apid: importing {repo}'s main from GitHub: {e}");
        }
        lane = export_lane(app, repo, &change, &t, &branch);
    }
    let lane = match lane {
        Ok(l) if l["state"] != "moved" => l,
        Ok(_) => return,
        Err(e) => return eprintln!("apid: pushing {change} to GitHub: {e}"),
    };
    let Some(sha) = lane["head"].as_str().map(str::to_string) else {
        return;
    };
    let pull = match pull_of_lane(&t, &change, &branch, &title) {
        Ok(p) => p,
        Err(e) => return eprintln!("apid: the pull request of {change}: {e}"),
    };
    let checks = match t.api.check_runs(&t.token, &t.owner, &t.name, &sha) {
        Ok(c) => c,
        Err(e) => return eprintln!("apid: the checks of {change}: {e}"),
    };
    let gate = gate_of(&checks);
    record_gate(app, repo, &change, &sha, &checks, gate);
    if pull.merged {
        land(
            app,
            repo,
            reference,
            &t,
            &change,
            &title,
            pull.merge_commit.as_deref(),
        );
    }
}

/// Pushes the lane to its branch (`ci`'s export, Weft's `lanes/{change}/export`).
fn export_lane(
    app: &App,
    repo: &str,
    change: &str,
    t: &Target,
    branch: &str,
) -> Result<Value, String> {
    app.weft.post(
        &format!("/repos/{repo}/lanes/{change}/export"),
        &json!({ "connection": t.connection, "branch": branch, "token": t.token }),
    )
}

/// The pull request of a lane's branch: the open one or the merged one, else a new one into the repo's main.
fn pull_of_lane(t: &Target, change: &str, branch: &str, title: &str) -> Result<Pull, String> {
    let pulls = t.api.pulls(&t.token, &t.owner, &t.name, branch)?;
    if let Some(p) = pulls.into_iter().find(|p| p.state == "open" || p.merged) {
        return Ok(p);
    }
    let body = format!(
        "AgentPlaza lane {change}. Its checks are GitHub's: the lane lands in AgentPlaza when this pull request merges."
    );
    t.api
        .open_pull(&t.token, &t.owner, &t.name, title, branch, &t.main, &body)
}

/// Records the gate of a lane's head commit as a run of the `ci` recipe, once per (commit, verdict): its checks are the
/// run's rows, and the run ends passed or failed. A pending gate is not recorded.
fn record_gate(app: &App, repo: &str, change: &str, sha: &str, checks: &[CheckRun], gate: Gate) {
    {
        let mut seen = app.weft.gates.lock().unwrap_or_else(|p| p.into_inner());
        if seen
            .get(change)
            .is_some_and(|(s, g)| s == sha && g == gate.name())
        {
            return;
        }
        seen.insert(
            change.to_string(),
            (sha.to_string(), gate.name().to_string()),
        );
    }
    if gate == Gate::Pending {
        return;
    }
    let (Some(project), Some(recipe)) = (
        project_of(app, repo),
        app.recipe_runs.recipe(crate::checks::RECIPE, None),
    ) else {
        return;
    };
    let now = apz_plaza::now_ms();
    let (run, _) = app.recipe_runs.start(
        &recipe,
        source_of(&recipe),
        TRIGGER,
        None,
        Some(project.id.as_str()),
        Some(change),
        now,
    );
    let failing: Vec<&str> = checks
        .iter()
        .filter(|c| !row_of(c).passed)
        .map(|c| c.name.as_str())
        .collect();
    app.recipe_runs.close_external(
        &run,
        checks.iter().map(row_of).collect(),
        gate == Gate::Passed,
        &format!("GitHub's checks on {sha} failed: {}", failing.join(", ")),
        now,
    );
    app.bus.publish(
        "github_gate",
        &json!({ "repo": repo, "change": change, "sha": sha, "gate": gate.name(), "run": run }),
    );
}

/// GitHub merged the lane: Weft lands it through its queue (the queue's own step, with the lane at its head). GitHub's
/// merge commit is then recorded as the lane's export to the connection: the connection's head from then on, and a
/// commit the import does not bring in again.
fn land(
    app: &App,
    repo: &str,
    reference: &str,
    t: &Target,
    change: &str,
    title: &str,
    merge: Option<&str>,
) {
    let Some(merge) = merge else {
        return;
    };
    match app.weft.post(
        &format!("/repos/{repo}/queue/step"),
        &json!({ "ref": reference }),
    ) {
        Ok(v) if v["outcome"] == "landed" => {
            if let Err(e) = app.weft.hub(
                "export",
                json!({ "repo": repo, "connection": t.connection, "change": change, "commits": [{ "intent": title, "git": merge }], "head": merge }),
            ) {
                eprintln!("apid: {change} landed, but {repo}'s GitHub head is not recorded: {e}");
            }
            app.bus.publish(
                "weft_land",
                &json!({ "repo": repo, "change": change, "revision": v["revision"] }),
            );
        }
        Ok(v) if v["outcome"] == "conflict" => crate::weft_hub::conflict_task(app, repo, &v),
        Ok(_) => {}
        Err(e) => eprintln!("apid: landing {change} on {repo}: {e}"),
    }
}

/// A workflow run's phase, as GitHub's check run shows it.
pub enum RunPhase<'a> {
    Started,
    Finished { passed: bool, summary: &'a str },
}

/// The check runs this hub created for workflow runs, by (run, workflow): a started run's check is completed when the run
/// ends. A hub that restarts forgets them, and a run that ends then gets a completed check of its own.
static RUN_CHECKS: Mutex<BTreeMap<(String, String), u64>> = Mutex::new(BTreeMap::new());

/// The most characters of a run's summary GitHub's output takes.
const SUMMARY_CHARS: usize = 1000;

/// The name of a workflow's check run on GitHub.
fn check_name(workflow: &str) -> String {
    format!("plaza / {workflow}")
}

/// A finished run's conclusion (GitHub's words).
fn conclusion_of(passed: bool) -> &'static str {
    if passed { "success" } else { "failure" }
}

/// A run's summary as GitHub's output takes it: its first characters.
fn summary_of(summary: &str) -> String {
    summary.chars().take(SUMMARY_CHARS).collect()
}

/// Reports a workflow run's phase as its check run on GitHub, on the commit the run's subject is gated at. Only a `ci`
/// project has one: a `mirror` project and a run without a change (a release) report nothing, and a change whose gate
/// is not recorded yet (its lane is not pushed and checked) reports nothing yet. Errors are returned for the caller to
/// print: the run does not fail on them.
pub fn report_run(
    app: &App,
    project: &str,
    subject: Option<&str>,
    run: &str,
    workflow: &str,
    phase: RunPhase<'_>,
) -> Result<(), String> {
    let p = app
        .projects
        .get(project)
        .ok_or_else(|| format!("no project {project}"))?;
    let Some(repo) = p.repo.as_deref() else {
        return Ok(());
    };
    if mode_of(app, repo) != CI {
        return Ok(());
    }
    let Some(change) = subject.and_then(|s| s.strip_prefix("change:")) else {
        return Ok(());
    };
    let gated = app
        .weft
        .gates
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(change)
        .map(|(sha, _)| sha.clone());
    let Some(sha) = gated else {
        return Ok(());
    };
    let t = target(app, repo)?;
    let mut open = RUN_CHECKS.lock().unwrap_or_else(|p| p.into_inner());
    report_on(&t, &sha, run, workflow, phase, &mut open)
}

/// Writes a run's phase on `sha` through `t`'s GitHub API: a started run creates its check (in progress) and remembers
/// its id in `open`; a finished run completes that check, or creates a completed one when none is remembered.
fn report_on(
    t: &Target,
    sha: &str,
    run: &str,
    workflow: &str,
    phase: RunPhase<'_>,
    open: &mut BTreeMap<(String, String), u64>,
) -> Result<(), String> {
    let key = (run.to_string(), workflow.to_string());
    let (name, api) = (check_name(workflow), &t.api);
    match phase {
        RunPhase::Started => {
            let check = CheckWrite {
                name,
                status: "in_progress".into(),
                conclusion: None,
                title: format!("plaza workflow {workflow}: running"),
                summary: format!("Run {run} is running."),
            };
            let id = api.create_check_run(&t.token, &t.owner, &t.name, sha, &check)?;
            open.insert(key, id);
        }
        RunPhase::Finished { passed, summary } => {
            let verdict = if passed { "passed" } else { "failed" };
            let summary = match summary_of(summary) {
                s if s.trim().is_empty() => format!("Run {run} {verdict}."),
                s => s,
            };
            let check = CheckWrite {
                name,
                status: "completed".into(),
                conclusion: Some(conclusion_of(passed).into()),
                title: format!("plaza workflow {workflow}: {verdict}"),
                summary,
            };
            match open.get(&key).copied() {
                Some(id) => {
                    api.update_check_run(&t.token, &t.owner, &t.name, id, &check)?;
                    open.remove(&key);
                }
                None => {
                    api.create_check_run(&t.token, &t.owner, &t.name, sha, &check)?;
                }
            }
        }
    }
    Ok(())
}

/// A push to GitHub's repository `full_name` (`owner/name`): every `ci` project whose GitHub connection is that repository
/// imports its main now, instead of on the next poll (21.6). Returns the repos it imported.
pub fn on_push(app: &App, full_name: &str) -> Vec<String> {
    let mut done = Vec::new();
    for project in app.projects.list() {
        if project.github != CI {
            continue;
        }
        let Some(repo) = project.repo.clone() else {
            continue;
        };
        let Ok(t) = target(app, &repo) else {
            continue;
        };
        if format!("{}/{}", t.owner, t.name) != full_name {
            continue;
        }
        match poll(app, &repo) {
            Ok(()) => done.push(repo),
            Err(e) => eprintln!("apid: importing {repo}'s main after a push: {e}"),
        }
    }
    done
}

/// Imports what landed on a `ci` repo's GitHub main since the last import (the catch-up pass polls for it).
pub fn poll(app: &App, repo: &str) -> Result<(), String> {
    let t = target(app, repo)?;
    // A lane in the queue has its own step (its gate and its landing) with the import in it: the poll waits for it.
    let queue = app.weft.get(&format!("/repos/{repo}/queue?ref=main"))?;
    if queue["queue"].as_array().is_some_and(|q| !q.is_empty()) {
        return Ok(());
    }
    sync_main(app, repo, &t)
}

/// GitHub's main moved (a merge that was not a lane): Weft imports what landed there and records the head it reached.
fn sync_main(app: &App, repo: &str, t: &Target) -> Result<(), String> {
    let report = app.weft.post(
        &format!("/repos/{repo}/import"),
        &json!({ "source": t.remote, "branch": t.main, "token": t.token, "platform": "github" }),
    )?;
    crate::review_graph::restore_carried(app, repo);
    let head = report["head"]
        .as_str()
        .ok_or("the import gave no head")?
        .to_string();
    app.weft.hub(
        "connection.state",
        json!({ "repo": repo, "id": t.connection, "state": "in-step", "remote_head": head }),
    )?;
    Ok(())
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platforms::http::{Body, Fake};
    use std::sync::Arc;

    fn check(name: &str, status: &str, conclusion: Option<&str>) -> CheckRun {
        CheckRun {
            name: name.into(),
            status: status.into(),
            conclusion: conclusion.map(str::to_string),
            html_url: String::new(),
        }
    }

    #[test]
    fn the_gate_is_failed_at_one_failure_passed_when_all_pass_and_pending_otherwise() {
        assert_eq!(gate_of(&[]), Gate::Pending, "no checks yet");
        assert_eq!(
            gate_of(&[
                check("lint", "completed", Some("success")),
                check("tests", "in_progress", None)
            ]),
            Gate::Pending
        );
        assert_eq!(
            gate_of(&[
                check("lint", "completed", Some("failure")),
                check("tests", "in_progress", None)
            ]),
            Gate::Failed,
            "a failure does not wait for the rest"
        );
        assert_eq!(
            gate_of(&[
                check("lint", "completed", Some("success")),
                check("docs", "completed", Some("skipped")),
                check("audit", "completed", Some("neutral"))
            ]),
            Gate::Passed
        );
        assert_eq!(
            gate_of(&[check("tests", "completed", Some("cancelled"))]),
            Gate::Failed
        );
    }

    #[test]
    fn a_check_run_is_a_row_that_passes_only_when_it_passed() {
        let ok = row_of(&check("lint", "completed", Some("success")));
        assert!(ok.passed && ok.summary == "success");
        let running = row_of(&check("tests", "in_progress", None));
        assert!(!running.passed && running.summary == "in_progress");
    }

    #[test]
    fn a_clone_url_is_its_owner_and_name() {
        assert_eq!(
            slug_of("https://github.com/JumpHash/agentplaza.git").unwrap(),
            ("JumpHash".to_string(), "agentplaza".to_string())
        );
        assert_eq!(
            slug_of("http://127.0.0.1:9/JumpHash/site/").unwrap(),
            ("JumpHash".to_string(), "site".to_string())
        );
        assert!(slug_of("site").is_err());
    }

    #[test]
    fn a_lane_is_pushed_to_its_own_branch() {
        assert_eq!(branch_of("w-12"), "agentplaza/w-12");
    }

    #[test]
    fn a_run_reports_its_check_name_conclusion_and_summary() {
        assert_eq!(check_name("build"), "plaza / build");
        assert_eq!(conclusion_of(true), "success");
        assert_eq!(conclusion_of(false), "failure");
        assert_eq!(summary_of("all green"), "all green");
        assert_eq!(summary_of(&"é".repeat(1500)).chars().count(), SUMMARY_CHARS);
    }

    /// The last JSON body the fake was sent.
    fn last_sent(fake: &Fake) -> Value {
        match fake.seen().last().map(|s| s.3.clone()) {
            Some(Some(Body::Json(v))) => v,
            _ => Value::Null,
        }
    }

    /// A target on `JumpHash/site` whose GitHub keeps check runs: a POST creates one (ids from 1), a PATCH updates it,
    /// a GET lists the runs of the commit it names.
    fn checks_api() -> (Target, Arc<Fake>) {
        let runs: Arc<Mutex<Vec<Value>>> = Arc::default();
        let store = Arc::clone(&runs);
        let fake = Arc::new(Fake::new(move |m, u, b| {
            let mut runs = store.lock().unwrap_or_else(|p| p.into_inner());
            let body = match b {
                Some(Body::Json(v)) => v.clone(),
                _ => Value::Null,
            };
            match m {
                "POST" => {
                    let id = runs.len() as u64 + 1;
                    runs.push(json!({
                        "id": id, "name": body["name"], "status": body["status"], "conclusion": body["conclusion"],
                        "output": body["output"], "head_sha": body["head_sha"], "html_url": "",
                    }));
                    (201, json!({ "id": id }))
                }
                "PATCH" => {
                    let id = u.rsplit('/').next().and_then(|n| n.parse::<u64>().ok());
                    match runs.iter_mut().find(|r| r["id"].as_u64() == id) {
                        Some(r) => {
                            r["status"] = body["status"].clone();
                            r["conclusion"] = body["conclusion"].clone();
                            r["output"] = body["output"].clone();
                            (200, json!({ "id": id }))
                        }
                        None => (404, json!({ "message": "Not Found" })),
                    }
                }
                "GET" => {
                    let sha = u
                        .split("/commits/")
                        .nth(1)
                        .and_then(|r| r.split('/').next())
                        .unwrap_or("");
                    let mine: Vec<&Value> = runs.iter().filter(|r| r["head_sha"] == sha).collect();
                    (200, json!({ "check_runs": mine }))
                }
                _ => (404, json!({ "message": "Not Found" })),
            }
        }));
        let target = Target {
            connection: "c1".into(),
            owner: "JumpHash".into(),
            name: "site".into(),
            main: "main".into(),
            remote: "https://github.com/JumpHash/site.git".into(),
            token: "ghs_1".into(),
            api: GitHub::new("https://github.com", fake.clone()),
        };
        (target, fake)
    }

    #[test]
    fn a_run_is_an_in_progress_check_that_the_gate_waits_on_until_its_end() {
        let (t, fake) = checks_api();
        let mut open = BTreeMap::new();
        let names = |sha: &str| t.api.check_runs("ghs_1", "JumpHash", "site", sha).unwrap();

        report_on(&t, "c0ffee", "r1", "build", RunPhase::Started, &mut open).unwrap();
        let created = last_sent(&fake);
        assert_eq!(
            (
                created["name"].as_str(),
                created["status"].as_str(),
                created["head_sha"].as_str(),
                created["conclusion"].is_null()
            ),
            (
                Some("plaza / build"),
                Some("in_progress"),
                Some("c0ffee"),
                true
            ),
            "{created}"
        );
        assert_eq!(open.len(), 1, "the started check is remembered");
        assert_eq!(gate_of(&names("c0ffee")), Gate::Pending);

        let long = "x".repeat(1500);
        report_on(
            &t,
            "c0ffee",
            "r1",
            "build",
            RunPhase::Finished {
                passed: false,
                summary: &long,
            },
            &mut open,
        )
        .unwrap();
        assert!(
            fake.urls().last().is_some_and(
                |u| u == "PATCH https://api.github.com/repos/JumpHash/site/check-runs/1"
            ),
            "the end updates the started check: {:?}",
            fake.urls()
        );
        let patched = last_sent(&fake);
        assert_eq!(patched["conclusion"], "failure");
        assert_eq!(patched["output"]["title"], "plaza workflow build: failed");
        assert_eq!(
            patched["output"]["summary"]
                .as_str()
                .map(|s| s.chars().count()),
            Some(SUMMARY_CHARS)
        );
        assert!(open.is_empty());
        assert_eq!(gate_of(&names("c0ffee")), Gate::Failed);

        // A passing run passes its gate; a run that ends without a started check creates a completed one.
        report_on(&t, "beef", "r2", "build", RunPhase::Started, &mut open).unwrap();
        assert_eq!(gate_of(&names("beef")), Gate::Pending);
        report_on(
            &t,
            "beef",
            "r2",
            "build",
            RunPhase::Finished {
                passed: true,
                summary: "all green",
            },
            &mut open,
        )
        .unwrap();
        assert_eq!(gate_of(&names("beef")), Gate::Passed);
        report_on(
            &t,
            "beef",
            "r3",
            "lint",
            RunPhase::Finished {
                passed: false,
                summary: "",
            },
            &mut open,
        )
        .unwrap();
        let orphan = last_sent(&fake);
        assert_eq!(
            (orphan["status"].as_str(), orphan["conclusion"].as_str()),
            (Some("completed"), Some("failure"))
        );
        assert_eq!(orphan["output"]["summary"], "Run r3 failed.");
        assert!(fake.urls().last().is_some_and(|u| u.starts_with("POST ")));
    }
}
