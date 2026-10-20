//! Project membership: the project each agent works on. An agent is on at most one project. One with none is on
//! the **bench** and does nothing:
//! - its work is held (`scheduler::dispatch`);
//! - new work is refused at the source (chat, messages, hires, assignment);
//! - its model and tool calls are refused even mid-turn (`internal`, the engine relays);
//! - it sleeps after a minute (`sleep`).
//!
//! Only the orchestrator works without a project: it opens the projects and runs them. A paused project holds its
//! members' work the same way. An archived or delivered project sends them back to the bench.
//!
//! The store is `members.json` under the state dir, saved with the plaza state (component `members`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agents::{Agent, Status};
use crate::settings::{Tier, tier_of};
use crate::state::App;

/// One agent's place: the project it works on, and since when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Membership {
    pub project: String,
    pub since_ms: u64,
}

/// Every agent's membership, by CID. An agent with no entry is on the bench.
pub struct Members {
    path: PathBuf,
    by_cid: Mutex<BTreeMap<u64, Membership>>,
}

impl Members {
    /// Loads the store; a missing or unreadable file starts empty (everyone on the bench).
    pub fn load(path: PathBuf) -> Self {
        let by_cid = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            path,
            by_cid: Mutex::new(by_cid),
        }
    }

    fn guard(&self) -> MutexGuard<'_, BTreeMap<u64, Membership>> {
        self.by_cid.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The project agent `cid` works on, or `None` on the bench.
    pub fn project_of(&self, cid: u64) -> Option<String> {
        self.guard().get(&cid).map(|m| m.project.clone())
    }

    /// Agent `cid`'s membership, with when it joined.
    pub fn of(&self, cid: u64) -> Option<Membership> {
        self.guard().get(&cid).cloned()
    }

    /// The agents on `project`, by CID.
    pub fn members(&self, project: &str) -> Vec<u64> {
        self.guard()
            .iter()
            .filter(|(_, m)| m.project == project)
            .map(|(cid, _)| *cid)
            .collect()
    }

    /// Puts `cid` on `project` (leaving any other), and writes the file; on failure nothing changes. Returns the
    /// project it left, if any.
    pub fn join(&self, cid: u64, project: &str, now_ms: u64) -> std::io::Result<Option<String>> {
        let mut by_cid = self.guard();
        let mut next = by_cid.clone();
        let left = next
            .insert(
                cid,
                Membership {
                    project: project.to_string(),
                    since_ms: now_ms,
                },
            )
            .map(|m| m.project)
            .filter(|p| p != project);
        self.save(&next)?;
        *by_cid = next;
        Ok(left)
    }

    /// Sends `cid` to the bench, and writes the file; returns the project it left.
    pub fn leave(&self, cid: u64) -> std::io::Result<Option<String>> {
        let mut by_cid = self.guard();
        if !by_cid.contains_key(&cid) {
            return Ok(None);
        }
        let mut next = by_cid.clone();
        let left = next.remove(&cid).map(|m| m.project);
        self.save(&next)?;
        *by_cid = next;
        Ok(left)
    }

    /// Sends every member of `project` to the bench (it was archived or delivered); returns their CIDs.
    pub fn clear(&self, project: &str) -> std::io::Result<Vec<u64>> {
        let mut by_cid = self.guard();
        let gone: Vec<u64> = by_cid
            .iter()
            .filter(|(_, m)| m.project == project)
            .map(|(cid, _)| *cid)
            .collect();
        if gone.is_empty() {
            return Ok(gone);
        }
        let mut next = by_cid.clone();
        next.retain(|_, m| m.project != project);
        self.save(&next)?;
        *by_cid = next;
        Ok(gone)
    }

    /// The whole state as JSON, for Postgres (`crate::db`); "" when there is nothing to save.
    pub fn snapshot(&self) -> String {
        let by_cid = self.guard();
        if by_cid.is_empty() {
            return String::new();
        }
        serde_json::to_string(&*by_cid).unwrap_or_default()
    }

    /// Replaces the state with a [`Members::snapshot`] and writes the file; a refusal changes nothing.
    pub fn restore(&self, json: &str) -> Result<(), String> {
        let restored: BTreeMap<u64, Membership> =
            serde_json::from_str(json).map_err(|e| format!("members: {e}"))?;
        let mut by_cid = self.guard();
        self.save(&restored).map_err(|e| format!("members: {e}"))?;
        *by_cid = restored;
        Ok(())
    }

    /// Writes the map to `members.json` atomically (tmp file, then rename).
    fn save(&self, by_cid: &BTreeMap<u64, Membership>) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(by_cid).map_err(std::io::Error::other)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path)
    }
}

/// Whether `role` works without a project: only the orchestrator, which opens the projects and runs them.
pub fn plaza_wide(role: &str) -> bool {
    matches!(tier_of(role), Tier::Orchestrator)
}

/// Where a project stands for its members' work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectState {
    /// Its members work.
    Active,
    /// The owner paused it: its members' work waits.
    Paused,
    /// Archived or delivered: its members go back to the bench.
    Over,
    /// No such project (it was removed): its members are as good as on the bench.
    Missing,
}

/// The reason an agent may not work now, or `None` when it may: the rule every gate applies. `project` is the
/// agent's membership and `state` its project's (`None` with no project).
pub fn hold_reason(
    cid: u64,
    role: &str,
    project: Option<&str>,
    state: Option<ProjectState>,
) -> Option<String> {
    if plaza_wide(role) {
        return None;
    }
    let Some(project) = project else {
        return Some(format!(
            "agent {cid} is on the bench: it works only on a project, so the owner has to add it to one first"
        ));
    };
    match state.unwrap_or(ProjectState::Missing) {
        ProjectState::Active => None,
        ProjectState::Paused => Some(format!(
            "project {project} is paused: its agents wait until the owner resumes it"
        )),
        ProjectState::Over => Some(format!(
            "project {project} is over (archived or delivered): agent {cid} is back on the bench"
        )),
        ProjectState::Missing => Some(format!(
            "project {project} no longer exists: agent {cid} is on the bench"
        )),
    }
}

/// The state of project `id` on the hub.
pub fn project_state(app: &App, id: &str) -> ProjectState {
    match app.projects.get(id) {
        None => ProjectState::Missing,
        // A delivered project in maintenance is still worked: its watchers sweep and the owner's picks run.
        Some(p) if p.archived || (p.stage == "delivered" && !p.in_maintenance()) => {
            ProjectState::Over
        }
        Some(p) if p.paused => ProjectState::Paused,
        Some(_) => ProjectState::Active,
    }
}

/// Why `agent` may not work now, or `None` when it may (see [`hold_reason`]).
pub fn held(app: &App, agent: &Agent) -> Option<String> {
    held_as(app, agent.cid, &agent.role)
}

/// Why agent `cid` of `role` may not work now, or `None` when it may.
pub fn held_as(app: &App, cid: u64, role: &str) -> Option<String> {
    if plaza_wide(role) {
        return None;
    }
    let project = app.members.project_of(cid);
    let state = project.as_deref().map(|p| project_state(app, p));
    hold_reason(cid, role, project.as_deref(), state)
}

/// Why agent `cid` may not work now (looked up on the roster), or `None` when it may. An agent the roster does not
/// know is held.
pub fn held_cid(app: &App, cid: u64) -> Option<String> {
    match app.agents.get(cid) {
        Some(agent) => held(app, &agent),
        None => Some(format!("no agent {cid}")),
    }
}

/// Checks that `agent` may take a task of `project` (`None`: a task outside projects): the task must belong to a
/// project, and the agent must work on it now. The orchestrator plans; it takes no task.
pub fn may_take_task(app: &App, agent: &Agent, project: Option<&str>) -> Result<(), String> {
    let Some(project) = project else {
        return Err(
            "the task belongs to no project: tasks run only inside a project (move it into one first)".into(),
        );
    };
    if plaza_wide(&agent.role) {
        return Err("the orchestrator plans work; give the task to a member of its project".into());
    }
    if let Some(why) = held(app, agent) {
        return Err(why);
    }
    match app.members.project_of(agent.cid) {
        Some(own) if own == project => Ok(()),
        Some(own) => Err(format!(
            "agent {} works on project {own}, not on {project}",
            agent.cid
        )),
        None => Err(format!("agent {} is on the bench", agent.cid)),
    }
}

/// Puts agent `cid` on `project`: checks both, leaves its old project (its work there stops), records the
/// membership, pushes its new profile and wakes it if its project has work for it. Returns the agent.
pub fn join_project(app: &App, cid: u64, project: &str) -> Result<Agent, (u16, String)> {
    let agent = app
        .agents
        .get(cid)
        .ok_or_else(|| (404, format!("no agent {cid}")))?;
    if agent.kind == "model" {
        return Err((
            409,
            "a model agent takes no work, so it joins no project".into(),
        ));
    }
    if plaza_wide(&agent.role) {
        return Err((
            409,
            "the orchestrator works on every project; it joins none".into(),
        ));
    }
    match project_state(app, project) {
        ProjectState::Missing => return Err((404, format!("no project {project}"))),
        ProjectState::Over => {
            return Err((
                409,
                format!("project {project} is over (archived or delivered)"),
            ));
        }
        ProjectState::Active | ProjectState::Paused => {}
    }
    if app.members.project_of(cid).as_deref() == Some(project) {
        return Ok(agent);
    }
    // Work for the old project (or for none) stops before the agent starts on the new one. Work for the new one stays:
    // a task of it the agent took before projects had members runs again once the agent is on it.
    release_except(
        app,
        cid,
        project,
        &format!("agent {cid} moved to project {project}"),
    );
    app.members
        .join(cid, project, apz_plaza::now_ms())
        .map_err(|e| (500, format!("membership not saved: {e}")))?;
    after_change(app, cid);
    Ok(app.agents.get(cid).unwrap_or(agent))
}

/// Sends agent `cid` to the bench: its work stops, its membership goes, its profile says so. Returns the project it
/// left.
pub fn leave_project(app: &App, cid: u64) -> Result<Option<String>, String> {
    release(app, cid, &format!("agent {cid} went back to the bench"));
    let left = app
        .members
        .leave(cid)
        .map_err(|e| format!("membership not saved: {e}"))?;
    after_change(app, cid);
    Ok(left)
}

/// Sends every member of `project` to the bench (archived, or delivered); returns their CIDs.
pub fn release_project(app: &App, project: &str) -> Vec<u64> {
    let members = app.members.members(project);
    for &cid in &members {
        release(app, cid, &format!("project {project} is over"));
    }
    let gone = match app.members.clear(project) {
        Ok(gone) => gone,
        Err(e) => {
            eprintln!("apid: members of {project} not released: {e}");
            return Vec::new();
        }
    };
    for &cid in &gone {
        after_change(app, cid);
    }
    gone
}

/// Stops what agent `cid` is doing for its project: its work in flight is cancelled and its queue is dropped, so
/// nothing it took on as a member runs after it leaves.
fn release(app: &App, cid: u64, reason: &str) {
    app.work.cancel(app, cid, reason);
    for item in app.work.queued(cid) {
        drop_waiting(app, cid, &item, reason);
    }
}

/// Takes waiting item `item` off agent `cid`: a task the agent claimed goes back to open, for another of its project's
/// agents (or, outside projects, to be moved into one) rather than failing; any other item ends as stopped. The item
/// in flight is not this one's: its cancel ends it, as the agent says.
fn drop_waiting(app: &App, cid: u64, item: &crate::scheduler::WorkItem, reason: &str) {
    let claimed_by_it = item
        .origin
        .task
        .as_deref()
        .and_then(|task| app.plaza.tasks.find(task))
        .is_some_and(|task| task.state() == "claimed" && task.view().agent_cid == Some(cid));
    if claimed_by_it
        && let Some(task) = item.origin.task.as_deref()
        && crate::scheduler::requeue_task(app, task, reason).is_ok()
    {
        return;
    }
    app.work.cancel_queued(app, cid, item.id, reason);
}

/// Stops agent `cid`'s work as [`release`] does, except its work for project `keep` (see [`keeps`]).
fn release_except(app: &App, cid: u64, keep: &str, reason: &str) {
    let project_of = |task: &str| {
        app.plaza
            .tasks
            .find(task)
            .and_then(|t| t.project().map(str::to_string))
    };
    if app
        .work
        .current(cid)
        .is_some_and(|item| !keeps(item.origin.task.as_deref(), keep, project_of))
    {
        app.work.cancel(app, cid, reason);
    }
    for item in app.work.queued(cid) {
        if !keeps(item.origin.task.as_deref(), keep, project_of) {
            drop_waiting(app, cid, &item, reason);
        }
    }
}

/// Whether work for `task` (None: work of no task) goes on when its agent joins `project`: only a task of `project`
/// does (`project_of` says a task's project); the rest was for the agent's old project, or for none.
fn keeps(task: Option<&str>, project: &str, project_of: impl Fn(&str) -> Option<String>) -> bool {
    task.and_then(project_of).as_deref() == Some(project)
}

/// After a membership change: the agent's profile (its project's place in its prompt) is pushed, the roster's
/// views see the change, and an idle member's waiting work is sent.
fn after_change(app: &App, cid: u64) {
    if let Some(agent) = app.agents.get(cid) {
        if agent.status != Status::Offline {
            crate::agents::push_profile(app, &agent);
        }
        app.bus.publish("agent", &crate::agents::view(app, &agent));
    }
    app.work.dispatch_idle(app);
}

/// The paragraph an agent's system prompt carries about its project, or about the bench.
pub fn prompt_section(app: &App, cid: u64, role: &str) -> Option<String> {
    if plaza_wide(role) {
        return None;
    }
    let Some(project) = app
        .members
        .project_of(cid)
        .and_then(|id| app.projects.get(&id))
    else {
        return Some(
            "You are on the bench: you have no project, so you do no work until the owner puts you on one.".into(),
        );
    };
    let goal: String = project.goal.chars().take(600).collect();
    Some(format!(
        "You work on project {} \"{}\". Its goal: {goal}\nWork only on this project: its tasks, its board, and its members.",
        project.id, project.title
    ))
}

// ---- Routes ----------------------------------------------------------------------------------------------------------

/// `/v1/projects/{id}/members…`: who works on a project; the owner adds an agent from the bench or sends it back.
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/projects/{id}/members", list_route)
        .doc(RouteDoc::new("List a project's agents").errors(&[404]))
        .post("/v1/projects/{id}/members", join_route)
        .doc(RouteDoc::new("Put an agent on a project").errors(&[400, 404, 409, 500]))
        .delete("/v1/projects/{id}/members/{cid}", leave_route)
        .doc(RouteDoc::new("Send a project's agent back to the bench").errors(&[400, 404, 500]))
        .get("/v1/projects/{id}/suggested-members", suggested_route)
        .doc(
            RouteDoc::new("Suggest a project's agents from the agents that hold its tasks")
                .errors(&[404]),
        )
}

fn list_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    if app.projects.get(id).is_none() {
        return Response::error(404, "no such project");
    }
    let members: Vec<Value> = app
        .members
        .members(id)
        .into_iter()
        .filter_map(|cid| app.agents.get(cid))
        .map(|agent| crate::agents::view(app, &agent))
        .collect();
    Response::json(200, &members)
}

#[derive(Deserialize)]
struct JoinBody {
    cid: u64,
}

/// Body: `{cid}`.
fn join_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let body: JoinBody = match req.json() {
        Ok(body) => body,
        Err(e) => return Response::error(400, &format!("bad body: {e}")),
    };
    match join_project(app, body.cid, p.get("id")) {
        Ok(agent) => {
            notify_project(app, p.get("id"));
            Response::json(200, &crate::agents::view(app, &agent))
        }
        Err((status, e)) => Response::error(status, &e),
    }
}

fn leave_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let Ok(cid) = p.get("cid").parse::<u64>() else {
        return Response::error(400, "cid must be a number");
    };
    let id = p.get("id");
    if app.projects.get(id).is_none() {
        return Response::error(404, "no such project");
    }
    if app.members.project_of(cid).as_deref() != Some(id) {
        return Response::error(404, &format!("agent {cid} is not on project {id}"));
    }
    match leave_project(app, cid) {
        Ok(_) => {
            notify_project(app, id);
            Response::json(200, &json!({"cid": cid, "project": Value::Null}))
        }
        Err(e) => Response::error(500, &e),
    }
}

/// `GET /v1/projects/{id}/suggested-members`: the agents that hold or held the project's tasks and are on no
/// project, each with those tasks (`tasks`). They are who worked on a project from before membership, so the owner
/// can put them on it in one step.
fn suggested_route(app: &App, _: &Request, p: &Params<'_>) -> Response {
    let Some(project) = app.projects.get(p.get("id")) else {
        return Response::error(404, "no such project");
    };
    let mut held: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    for id in &project.tasks {
        if let Some(cid) = app
            .plaza
            .tasks
            .find(id)
            .and_then(|task| task.view().agent_cid)
        {
            held.entry(cid).or_default().push(id.clone());
        }
    }
    let rows: Vec<Value> = held
        .into_iter()
        .filter_map(|(cid, tasks)| {
            let agent = app.agents.get(cid)?;
            let free = agent.kind != "model"
                && !plaza_wide(&agent.role)
                && app.members.project_of(cid).is_none();
            free.then(|| {
                let mut view = crate::agents::view(app, &agent);
                view["tasks"] = json!(tasks);
                view
            })
        })
        .collect();
    Response::json(200, &rows)
}

/// Publishes the project again, so the consoles read its new members.
fn notify_project(app: &App, id: &str) {
    if let Some(project) = app.projects.get(id) {
        app.bus.publish("project", &project);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> Members {
        let dir = std::env::temp_dir().join(format!("apid-members-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Members::load(dir.join("members.json"))
    }

    #[test]
    fn joining_keeps_only_the_work_for_the_project_joined() {
        let project_of = |task: &str| match task {
            "t1" => Some("pr1".to_string()),
            "t2" => Some("pr2".to_string()),
            _ => None,
        };
        assert!(
            keeps(Some("t1"), "pr1", project_of),
            "a task of the project joined runs again"
        );
        assert!(
            !keeps(Some("t2"), "pr1", project_of),
            "another project's task stops"
        );
        assert!(
            !keeps(Some("t9"), "pr1", project_of),
            "a task of no project stops"
        );
        assert!(!keeps(None, "pr1", project_of), "work of no task stops");
    }

    #[test]
    fn an_agent_is_on_one_project_and_moving_leaves_the_old_one() {
        let members = store("move");
        assert_eq!(members.project_of(7), None, "everyone starts on the bench");
        assert_eq!(members.join(7, "pr1", 10).unwrap(), None);
        assert_eq!(members.join(8, "pr1", 11).unwrap(), None);
        assert_eq!(members.project_of(7).as_deref(), Some("pr1"));
        assert_eq!(members.join(7, "pr2", 12).unwrap().as_deref(), Some("pr1"));
        assert_eq!(members.members("pr1"), [8]);
        assert_eq!(members.members("pr2"), [7]);
        assert_eq!(members.leave(8).unwrap().as_deref(), Some("pr1"));
        assert_eq!(
            members.leave(8).unwrap(),
            None,
            "leaving the bench changes nothing"
        );
        assert_eq!(members.clear("pr2").unwrap(), [7]);
        assert!(members.snapshot().is_empty(), "nobody is on a project");
    }

    #[test]
    fn memberships_survive_a_restart_and_a_snapshot() {
        let members = store("restart");
        members.join(3, "pr4", 99).unwrap();
        let again = Members::load(members.path.clone());
        assert_eq!(
            again.of(3),
            Some(Membership {
                project: "pr4".into(),
                since_ms: 99
            })
        );
        let other = store("restore");
        other.restore(&members.snapshot()).unwrap();
        assert_eq!(other.project_of(3).as_deref(), Some("pr4"));
        assert!(other.restore("not json").is_err());
        assert_eq!(
            other.project_of(3).as_deref(),
            Some("pr4"),
            "a refused restore changes nothing"
        );
    }

    #[test]
    fn only_the_orchestrator_works_without_a_project() {
        assert_eq!(hold_reason(1, "orchestrator", None, None), None);
        let bench = hold_reason(2, "builder", None, None).expect("a bench agent is held");
        assert!(bench.contains("on the bench"), "{bench}");
        for role in ["mayor", "reviewer", "finance", "researcher"] {
            assert!(
                hold_reason(3, role, None, None).is_some(),
                "{role} needs a project"
            );
        }
        assert_eq!(
            hold_reason(4, "builder", Some("pr1"), Some(ProjectState::Active)),
            None
        );
        let paused = hold_reason(4, "builder", Some("pr1"), Some(ProjectState::Paused)).unwrap();
        assert!(paused.contains("paused"), "{paused}");
        assert!(hold_reason(4, "builder", Some("pr1"), Some(ProjectState::Over)).is_some());
        assert!(hold_reason(4, "builder", Some("pr9"), Some(ProjectState::Missing)).is_some());
    }
}
