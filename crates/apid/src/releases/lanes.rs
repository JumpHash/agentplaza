//! Release lanes and backports (PLAN 22.3). A maintenance line (`release/1.4`) is a Weft ref cut at a published release
//! (`release.lane`, hub-signed). A fix that landed on main is backported to the line: its files are merged onto the
//! line's versions three ways (Weft's line merge, `weft::merge::merge_text`: the base is the fix's base, ours the fix,
//! theirs the line's head), and a file the merge cannot settle refuses the backport with its path. The merged change is
//! opened on the line and written by a worker's agent (`revision.draft` and `revision.seal`, signed with the agent's own
//! key). Its task is bound to it and the review chain starts its review (4.2, `review_graph::start`, through [`Chain`]);
//! the line's queue lands it once that review has passed. A patch release is then drafted from the line (`flow::draft`
//! with `lane`), built, checked and published as any release is.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::{Value, json};
use weft::merge::merge_text;
use weft::model::Delta;

use super::entity::Release;
use super::flow::{self, Deps};
use crate::review_graph::RunState;

/// Signs a statement as an agent with the agent's own key. Production signs with `weft_hub::sign_as_agent`.
pub trait Agents {
    fn sign(&self, worker: u64, kind: &str, body: Value) -> Result<Value, String>;
}

/// The review chain a backport's change goes through (4.2). Production is `routes::AppChain`: the plaza's tasks, Weft's
/// lane bindings and `review_graph::start`; the tests give a fake.
pub trait Chain {
    /// A task for the backport, claimed by `worker`; its id.
    fn task(&self, project: &str, title: &str, worker: u64) -> Result<String, String>;
    /// Binds the task to the change that carries its work.
    fn bind(&self, task: &str, change: &str);
    /// Starts the review of the task's sealed change: the review run's id, when one started.
    fn review(&self, task: &str) -> Option<String>;
}

/// Opens the review of an adopted change (21.6): a remote's own commits, adopted by Weft as a lane. The planner's task is
/// bound to the change and its review starts through the chain, as a backport's does; [`land_backport`] lands the change
/// once that review passed. Returns the review run's id.
pub fn review_adoption(
    chain: &dyn Chain,
    project: &str,
    change: &str,
    planner: u64,
    title: &str,
) -> Result<String, String> {
    let task = chain.task(project, &format!("Review the adoption: {title}"), planner)?;
    chain.bind(&task, change);
    chain
        .review(&task)
        .ok_or_else(|| format!("the review of {change} did not start"))
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
    /// The task that carries the change through the review chain.
    pub task: String,
    /// The review run of the change (4.2).
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

/// Backports a fix that landed on main to a release line. The fix's files are merged onto the line (a clash refuses the
/// backport, naming its paths), the merged change is opened on the line and written by `worker`'s agent, and its review
/// starts through the chain. It lands with [`land_backport`] once the review has passed.
pub fn backport(
    d: &Deps,
    agents: &dyn Agents,
    chain: &dyn Chain,
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
    let Some(head) = refs[line].as_str() else {
        return Err(format!(
            "{repo} has no {line}: cut the line from its release first"
        ));
    };
    let landed = detail["landed"]
        .as_str()
        .ok_or_else(|| format!("{fix} has no landed revision"))?;
    let revision = d.weft.read(&format!("/revisions/{landed}"))?;
    let base = text(&revision["base"]);
    let deltas: Vec<Delta> = serde_json::from_value(revision["deltas"].clone())
        .map_err(|e| format!("{fix}: its landed revision's files do not read: {e}"))?;
    let merged = merge_onto_line(d, fix, &base, &deltas, head)?;
    let title = text(&detail["title"]);
    let title = format!("Backport: {title}");
    let task = chain.task(project, &title, worker)?;
    let opened = d.weft.post(
        "/lanes",
        &json!({
            "repo": repo, "ref": line, "task": task, "title": title,
            "author": { "kind": "agent", "cid": worker }, "kind": "normal",
        }),
    )?;
    let change = text(&opened["change"]);
    chain.bind(&task, &change);
    let body = serde_json::to_value(&merged).map_err(|e| e.to_string())?;
    agents.sign(
        worker,
        "revision.draft",
        json!({ "change": change, "n": 1, "deltas": body, "tool_calls": ["backport#1"], "turn": 1, "model": "backport" }),
    )?;
    agents.sign(
        worker,
        "revision.seal",
        json!({ "change": change, "n": 1, "deltas": body, "model": "backport" }),
    )?;
    let review_run = chain
        .review(&task)
        .ok_or_else(|| format!("the review of {change} did not start"))?;
    Ok(Backport {
        fix: fix.to_string(),
        change,
        line: line.to_string(),
        task,
        review_run,
        state: "reviewing".into(),
    })
}

/// The fix's files, as the line receives them: each file the fix changed is merged three ways (base: the fix's base;
/// ours: the fix; theirs: the line's head). A file the line did not change takes the fix's content; a file both changed
/// merges by lines (Weft's `merge_text`); a clash the merge cannot settle, a binary file or a delete the line changed is
/// named, and the backport is refused with the paths.
fn merge_onto_line(
    d: &Deps,
    fix: &str,
    base: &str,
    deltas: &[Delta],
    head: &str,
) -> Result<Vec<Delta>, String> {
    let before = listing(d, base)?;
    let theirs = listing(d, head)?;
    let mut out = Vec::new();
    let mut clashes = Vec::new();
    for delta in deltas {
        let path = &delta.path;
        let old = match before.contains(path) {
            true => Some(read_at(d, base, path)?),
            false => None,
        };
        let line = match theirs.contains(path) {
            true => Some(read_at(d, head, path)?),
            false => None,
        };
        let ours = match &delta.after {
            Some(hash) => Some(d.weft.blob(hash)?),
            None => None,
        };
        if line == old {
            out.push(delta.clone());
            continue;
        }
        if line == ours {
            continue;
        }
        let merged = match (&old, &ours, &line) {
            (Some(o), Some(f), Some(l))
                if weft::diff::is_text(o) && weft::diff::is_text(f) && weft::diff::is_text(l) =>
            {
                merge_text(
                    &String::from_utf8_lossy(o),
                    &String::from_utf8_lossy(f),
                    &String::from_utf8_lossy(l),
                )
                .ok()
            }
            _ => None,
        };
        match merged {
            Some(text) => out.push(Delta {
                path: path.clone(),
                after: Some(d.weft.upload(text.as_bytes())?),
                mode: delta.mode,
            }),
            None => clashes.push(path.clone()),
        }
    }
    if clashes.is_empty() {
        Ok(out)
    } else {
        Err(format!(
            "backporting {fix} to the line conflicts with it in: {}",
            clashes.join(", ")
        ))
    }
}

/// The paths a revision holds.
fn listing(d: &Deps, rev: &str) -> Result<BTreeSet<String>, String> {
    let files = d.weft.read(&format!("/revisions/{rev}/files"))?;
    Ok(files["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["path"].as_str().map(str::to_string))
        .collect())
}

/// A file's bytes at a revision (one the revision holds).
fn read_at(d: &Deps, rev: &str, path: &str) -> Result<Vec<u8>, String> {
    d.weft.bytes(&format!(
        "/revisions/{rev}/file?path={}",
        weft::server::percent_encode(path)
    ))
}

/// Lands a backport whose review passed: it is queued on its line, and the line's queue steps until it has landed.
pub fn land_backport(d: &Deps, change: &str) -> Result<Landing, String> {
    let review = d
        .reviews
        .runs_of_change(change)
        .into_iter()
        .last()
        .ok_or_else(|| format!("{change} has no backport review"))?;
    match review.state {
        RunState::Passed => {}
        RunState::Running | RunState::Rework => {
            return Err(format!("the review of {change} has not passed yet"));
        }
        RunState::Owner => return Err(format!("the review of {change} waits for the owner")),
        RunState::Failed => return Err(format!("the review of {change} did not pass")),
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
