//! Release lanes and backports (PLAN 22.3). A maintenance line (`release/1.4`) is a Weft ref cut at a published release
//! (`release.lane`, hub-signed). A fix that landed on main is backported to the line: a change opened on the line and
//! written by a worker's agent (`revision.draft` and `revision.seal`, signed with the agent's own key), reviewed as a
//! run of the `review` recipe, and landed through the line's queue like any change. A patch release is then drafted from
//! the line (`flow::draft` with `lane`), built, checked and published as any release is.
//!
//! The backport's review is the review recipe's run with trigger `backport`. The review graph (review_graph/) starts its
//! runs for tasks, not for backports, so until it is wired to them a backport waits in `reviewing`; the run's outcome is
//! what releases it (`Runs::finish_subject`).

use serde::Serialize;
use serde_json::{Value, json};

use super::entity::Release;
use super::flow::{self, Deps};
use crate::recipes::runs::RunState;

/// Signs a statement as an agent with the agent's own key. Production signs with `weft_hub::sign_as_agent`.
pub trait Agents {
    fn sign(&self, worker: u64, kind: &str, body: Value) -> Result<Value, String>;
}

/// A backport, from the fix to the line's queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Backport {
    /// The fix, as it landed on main.
    pub fix: String,
    /// The change on the line that carries the fix.
    pub change: String,
    /// The release line it lands on, `release/1.4`.
    pub line: String,
    /// The review recipe's run of the change (trigger `backport`).
    pub review_run: String,
    /// `reviewing` until the review passes.
    pub state: String,
}

/// A backport that landed on its line, and the line's head after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Landing {
    pub change: String,
    pub line: String,
    pub head: String,
}

/// The release line of a release: its own lane, or the `release/<major>.<minor>` of a release of main.
pub fn line_of(release: &Release) -> String {
    if release.lane != "main" {
        return release.lane.clone();
    }
    line_of_version(&release.version)
}

/// The line of a version as Weft names it: `release/1.4` for `v1.4.0`.
pub fn line_of_version(version: &str) -> String {
    let v = version.trim_start_matches('v');
    let mut parts = v.split('.');
    match (parts.next(), parts.next()) {
        (Some(major), Some(minor)) => format!("release/{major}.{minor}"),
        _ => format!("release/{v}"),
    }
}

fn text(v: &Value) -> String {
    v.as_str().map(str::to_string).unwrap_or_default()
}

/// Cuts a published release's line at the revision the release was cut from. Weft refuses a release that is not
/// published; cutting again is a no-op while the line still starts there.
pub fn cut_lane(d: &Deps, id: &str) -> Result<Release, String> {
    let rel = flow::get(d, id)?;
    d.weft.statement(
        "release.lane",
        json!({ "repo": rel.repo, "version": rel.version }),
    )?;
    flow::refresh(d, id)
}

/// Backports a fix that landed on main to a release line. The change on the line is opened, its revision is written by
/// `worker`'s agent, and its review run starts. It lands with [`land_backport`] once the review has passed.
pub fn backport(
    d: &Deps,
    agents: &dyn Agents,
    project: &str,
    line: &str,
    fix: &str,
    worker: u64,
) -> Result<Backport, String> {
    if !line.starts_with("release/") {
        return Err(format!("{line} is not a release line (release/<line>)"));
    }
    let detail = d.weft.read(&format!("/changes/{fix}"))?;
    if detail["state"].as_str() != Some("landed") {
        return Err(format!(
            "{fix} has not landed: only a fix that landed on main is backported"
        ));
    }
    if detail["ref"].as_str() != Some("main") {
        return Err(format!(
            "{fix} landed on {}, not on main",
            text(&detail["ref"])
        ));
    }
    let repo = text(&detail["repo"]);
    let refs = d.weft.read(&format!("/repos/{repo}"))?["refs"].clone();
    if refs[line].as_str().is_none() {
        return Err(format!(
            "{repo} has no {line}: cut the line from its release first"
        ));
    }
    let landed = detail["landed"]
        .as_str()
        .ok_or_else(|| format!("{fix} has no landed revision"))?;
    let deltas = d.weft.read(&format!("/revisions/{landed}"))?["deltas"].clone();
    let title = text(&detail["title"]);
    let opened = d.weft.post(
        "/lanes",
        &json!({
            "repo": repo, "ref": line, "title": format!("Backport: {title}"),
            "author": { "kind": "agent", "cid": worker }, "kind": "normal",
        }),
    )?;
    let change = text(&opened["change"]);
    agents.sign(
        worker,
        "revision.draft",
        json!({ "change": change, "n": 1, "deltas": deltas, "tool_calls": ["backport#1"], "turn": 1, "model": "backport" }),
    )?;
    agents.sign(
        worker,
        "revision.seal",
        json!({ "change": change, "n": 1, "deltas": deltas, "model": "backport" }),
    )?;
    let review_run = d
        .runs
        .start_engine(
            "review",
            "backport",
            None,
            Some(project),
            Some(&change),
            apz_plaza::now_ms(),
        )
        .ok_or("no review recipe is on the hub")?;
    Ok(Backport {
        fix: fix.to_string(),
        change,
        line: line.to_string(),
        review_run,
        state: "reviewing".into(),
    })
}

/// Lands a backport whose review passed: it is queued on its line, and the line's queue steps until it has landed.
pub fn land_backport(d: &Deps, project: &str, change: &str) -> Result<Landing, String> {
    let review = d
        .runs
        .list(None, Some(project))
        .into_iter()
        .find(|v| v.run.trigger == "backport" && v.run.subject.as_deref() == Some(change))
        .ok_or_else(|| format!("{change} has no backport review"))?;
    match review.run.state {
        RunState::Passed => {}
        RunState::Running => return Err(format!("the review of {change} has not passed yet")),
        RunState::Owner => return Err(format!("the review of {change} waits for the owner")),
        RunState::Failed => return Err(format!("the review of {change} did not pass")),
        RunState::Cancelled => return Err(format!("the review of {change} was cancelled")),
    }
    let detail = d.weft.read(&format!("/changes/{change}"))?;
    if detail["state"].as_str() == Some("landed") {
        return Err(format!("{change} has landed already"));
    }
    let repo = text(&detail["repo"]);
    let line = text(&detail["ref"]);
    d.weft.statement(
        "queue.add",
        json!({ "repo": repo, "ref": line, "change": change, "n": 1 }),
    )?;
    let path = format!("/repos/{repo}/queue/step");
    // The queue lands its items in order: a step that lands another change first is followed by the next.
    for _ in 0..8 {
        let step = d.weft.post(&path, &json!({ "ref": line }))?;
        match step["outcome"].as_str() {
            Some("landed") if step["change"].as_str() == Some(change) => {
                return Ok(Landing {
                    change: change.to_string(),
                    line,
                    head: text(&step["revision"]),
                });
            }
            Some("landed") => continue,
            _ => return Err(format!("{change} did not land on {line}: {step}")),
        }
    }
    Err(format!(
        "{change} did not land on {line} within the queue's steps"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_line_is_named_by_its_minor_version() {
        assert_eq!(line_of_version("1.4.0"), "release/1.4");
        assert_eq!(line_of_version("v0.9.12"), "release/0.9");
        assert_eq!(line_of_version("2"), "release/2");
    }
}
