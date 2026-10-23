//! The plaza's domain state (apz-plaza) and its REST routes: tasks and the board.
//!
//! OWNER: slice "plaza-api". The handlers port noded's former ones — see
//! `git show 82d6286:crates/noded/src/lib.rs` (fn list_tasks … fn withdraw_task).

use std::sync::Arc;

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_plaza::ConnectedAgent;
use apz_plaza::events::EventBus;
use apz_plaza::social::{self, PostRequest, ReadRequest, Social};
use apz_plaza::tasks::{
    ClaimError, ClaimRequest, CreateTask, FinishRequest, Task, TaskLedger, TasksView,
};
use apz_proto::wire::WorkKind;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::agents::{self, Status};
use crate::owner_board::ProjectIndex;
use crate::scheduler::Origin;
use crate::state::App;

pub struct Plaza {
    pub events: Arc<EventBus>,
    pub tasks: TaskLedger,
    pub social: Social,
}

impl Plaza {
    /// A plaza with no tasks and an empty board.
    pub fn new() -> Self {
        let events = EventBus::new();
        Self {
            social: Social::new(Arc::clone(&events)),
            tasks: TaskLedger::new(Arc::clone(&events)),
            events,
        }
    }
}

/// `/v1/tasks…`, `/v1/board…`.
pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/tasks", list_tasks)
        .doc(RouteDoc::new("List the tasks").errors(&[400]))
        .post("/v1/tasks", |app, req, _| create_task(app, &req.body))
        .doc(
            RouteDoc::new("Create a task")
                .status(201)
                .errors(&[400, 404, 409]),
        )
        .get("/v1/tasks/{id}", |app, _, p| get_task(app, p.get("id")))
        .doc(RouteDoc::new("Get a task").errors(&[404]))
        .route("PATCH", "/v1/tasks/{id}", |app, req, p| {
            relink_task(app, p.get("id"), &req.body)
        })
        .doc(RouteDoc::new("Relink an open task").errors(&[400, 404, 409]))
        .post("/v1/tasks/{id}/move", |app, req, p| {
            move_task(app, p.get("id"), &req.body)
        })
        .doc(
            RouteDoc::new(
                "Move a task outside projects into a project, with the tasks linked to it",
            )
            .errors(&[400, 404, 409]),
        )
        .delete("/v1/tasks/{id}", |app, _, p| {
            withdraw_task(app, p.get("id"))
        })
        .doc(RouteDoc::new("Withdraw an open task").errors(&[404]))
        .post("/v1/tasks/{id}/claim", |app, req, p| {
            claim_task(app, p.get("id"), &req.body)
        })
        .doc(RouteDoc::new("Claim a task for a connected agent").errors(&[400, 404, 409]))
        .post("/v1/tasks/{id}/finish", |app, req, p| {
            finish_task(app, p.get("id"), &req.body)
        })
        .doc(RouteDoc::new("Finish a claimed task").errors(&[400, 404]))
        .get("/v1/board", |app, req, _| board_view(app, req))
        .doc(RouteDoc::new("Read the board").errors(&[400]))
        .post("/v1/board", |app, req, _| board_post(app, &req.body))
        .doc(
            RouteDoc::new("Post to the board")
                .status(201)
                .errors(&[400, 404, 409]),
        )
        .post("/v1/board/{id}/comments", |app, req, p| {
            board_comment(app, p.get("id"), &req.body)
        })
        .doc(
            RouteDoc::new("Comment on a board post")
                .status(201)
                .errors(&[400]),
        )
        .post("/v1/board/read", |app, req, _| board_read(app, &req.body))
        .doc(RouteDoc::new("Mark the board read up to a point").errors(&[400, 404, 409]))
}

/// Parses a JSON body into `T`, or answers 400 naming the request kind.
pub(crate) fn parse<T: DeserializeOwned>(body: &[u8], what: &str) -> Result<T, Response> {
    serde_json::from_slice(body)
        .map_err(|err| Response::error(400, &format!("invalid {what} request: {err}")))
}

/// The connected agent for a guest CID. Unknown is a 404; a known agent that
/// is offline is the ledger's own `AgentNotConnected` (a 409), so an
/// unreachable agent is refused before any task is touched.
fn connected(app: &App, cid: u64) -> Result<ConnectedAgent, Response> {
    let Some(agent) = app.agents.get(cid) else {
        return Err(Response::error(
            404,
            &format!("no agent with guest CID {cid}"),
        ));
    };
    if agent.status == Status::Offline {
        let refused = ClaimError::AgentNotConnected { cid };
        return Err(Response::error(refused.status(), &refused.to_string()));
    }
    Ok(agents::connected_agent(&agent))
}

/// Checks that connected agent `cid` may take a task of `project` (see `projects::may_take_task`): a 409 when it
/// may not.
fn takes(app: &App, cid: u64, project: Option<&str>) -> Result<(), Response> {
    let Some(agent) = app.agents.get(cid) else {
        return Err(Response::error(
            404,
            &format!("no agent with guest CID {cid}"),
        ));
    };
    crate::projects::may_take_task(app, &agent, project).map_err(|why| Response::error(409, &why))
}

/// The prompt an assigned task is worked from: its goal, and its acceptance
/// line when one is stated.
fn task_prompt(goal: &str, acceptance: Option<&str>) -> String {
    match acceptance {
        Some(acceptance) if !acceptance.trim().is_empty() => {
            format!("{goal}\n\nAcceptance: {acceptance}")
        }
        _ => goal.to_string(),
    }
}

/// `POST /v1/tasks` may name an `assignee`: the guest CID the task is claimed
/// for and queued to. Absent, the task is created open, as before.
#[derive(Debug, Deserialize)]
struct Assignee {
    #[serde(default)]
    assignee: Option<u64>,
}

/// `GET /v1/tasks`: every task, oldest first. `?project=<id>` keeps the project's tasks
/// ([`project_tasks`]), and `?ready=true` (or `false`) the ones that can (or cannot) be claimed now.
fn list_tasks(app: &App, req: &Request, _: &Params<'_>) -> Response {
    let ready = match req.query_param("ready") {
        None => None,
        Some("true") => Some(true),
        Some("false") => Some(false),
        Some(other) => {
            return Response::error(400, &format!("ready must be true or false, not {other:?}"));
        }
    };
    let tasks = match req.query_param("project") {
        Some(pid) => project_tasks(app, pid),
        None => app.plaza.tasks.list(),
    };
    let view = TasksView {
        tasks: tasks
            .iter()
            .map(|task| task.view())
            .filter(|view| ready.is_none_or(|ready| view.ready == ready))
            .collect(),
    };
    Response::json(200, &view)
}

/// The ledger's tasks for project `pid`, oldest first: the ones that name it, and the ones its record
/// lists (a task posted before tasks named their project is only listed there). None for a project
/// that does not exist and that no task names.
pub(crate) fn project_tasks(app: &App, pid: &str) -> Vec<Arc<Task>> {
    let listed = app
        .projects
        .get(pid)
        .map(|project| project.tasks)
        .unwrap_or_default();
    app.plaza
        .tasks
        .list()
        .into_iter()
        .filter(|task| task.project() == Some(pid) || listed.iter().any(|id| id == task.id()))
        .collect()
}

/// `POST /v1/tasks`: creates a task in `open`. With an `assignee`, the
/// assignee is checked first (so a refusal leaves the ledger untouched), the
/// task is claimed for it, and the assignee's work queue gets the task.
///
/// `after` names the tasks it runs after: it is handed out once they are all done, so an assignee is refused
/// while any of them is not. `project` is required: every task belongs to a project whose plan is approved
/// (implementation or review, as for an agent's `plaza_post_task`) and that is not archived. The task is linked
/// there, its `after` stays inside that project, and an assignee must work on it.
pub(crate) fn create_task(app: &App, body: &[u8]) -> Response {
    let request: CreateTask = match parse(body, "create") {
        Ok(request) => request,
        Err(refused) => return refused,
    };
    let assignee = match parse::<Assignee>(body, "create") {
        Ok(assignee) => assignee.assignee,
        Err(refused) => return refused,
    };
    let assignee = match assignee {
        Some(cid) => match connected(app, cid) {
            Ok(agent) => Some((cid, agent)),
            Err(refused) => return refused,
        },
        None => None,
    };
    if assignee.is_some() {
        let waiting = app.plaza.tasks.not_done(&request.after);
        if !waiting.is_empty() {
            let verb = if waiting.len() == 1 { "is" } else { "are" };
            return Response::error(
                409,
                &format!(
                    "the assignee cannot start it yet: it runs after {}, which {verb} not done",
                    waiting.join(", ")
                ),
            );
        }
    }
    let Some(pid) = request.project.as_deref().filter(|p| !p.trim().is_empty()) else {
        return Response::error(
            400,
            "a task belongs to a project: name it with `project` (POST /v1/projects opens one)",
        );
    };
    let Some(project) = app.projects.get(pid) else {
        return Response::error(404, &format!("no project {pid}"));
    };
    if project.archived {
        return Response::error(
            409,
            &format!("project {pid} is archived: it takes no new tasks"),
        );
    }
    if !matches!(project.stage.as_str(), "implementation" | "review") {
        return Response::error(
            409,
            &format!(
                "project {pid} is at {}: tasks can be posted only once its plan is approved",
                project.stage
            ),
        );
    }
    if let Some((cid, _)) = &assignee
        && let Err(refused) = takes(app, *cid, Some(pid))
    {
        return refused;
    }
    let task = match app.plaza.tasks.create(&request) {
        Ok(task) => task,
        Err(err) => return Response::error(err.status(), &err.to_string()),
    };
    if let Some(pid) = task.project()
        && let Err(refused) = app.projects.link_task(pid, task.id())
    {
        return Response::error(409, &refused);
    }
    if let Some((cid, agent)) = assignee {
        if let Err(refused) = task.claim(&agent) {
            return Response::error(refused.status(), &refused.to_string());
        }
        let id = task.id().to_string();
        let prompt = task_prompt(task.goal(), task.acceptance());
        let (prompt, origin) = crate::weft_hub::task_work(app, &task, cid, prompt);
        app.work.enqueue(
            app,
            cid,
            WorkKind::Task,
            format!("task-{id}"),
            prompt,
            origin,
        );
    }
    Response::json(201, &task.view())
}

/// The `PATCH /v1/tasks/{id}` body: the whole new list of the tasks it runs after.
#[derive(Debug, Deserialize)]
struct Relink {
    after: Vec<String>,
}

/// `PATCH /v1/tasks/{id}` `{after}`: replaces an open task's links. They are held to the rules a
/// create holds them to, and links that would close a cycle are a 400 that names it; a task that is
/// no longer open keeps its links (409).
fn relink_task(app: &App, id: &str, body: &[u8]) -> Response {
    if app.plaza.tasks.find(id).is_none() {
        return Response::error(404, &format!("no task with id {id}"));
    }
    let request: Relink = match parse(body, "relink") {
        Ok(request) => request,
        Err(refused) => return refused,
    };
    match app.plaza.tasks.relink(id, &request.after) {
        Ok(task) => Response::json(200, &task.view()),
        Err(refused) => Response::error(refused.status(), &refused.to_string()),
    }
}

/// The `POST /v1/tasks/{id}/move` body: the project the task goes to.
#[derive(Debug, Deserialize)]
struct MoveRequest {
    project: String,
}

/// `POST /v1/tasks/{id}/move` `{project}`: files a task outside projects under a project whose plan is approved and
/// that is not archived, with every task linked to it (links stay inside a project). A task outside projects never
/// runs, so this is how one from before projects gets done. Answers `{project, moved}`: the tasks moved, oldest
/// first.
fn move_task(app: &App, id: &str, body: &[u8]) -> Response {
    let request: MoveRequest = match parse(body, "move") {
        Ok(request) => request,
        Err(refused) => return refused,
    };
    let pid = request.project.trim();
    if pid.is_empty() {
        return Response::error(400, "name the project the task moves to");
    }
    let Some(project) = app.projects.get(pid) else {
        return Response::error(404, &format!("no project {pid}"));
    };
    if project.archived {
        return Response::error(
            409,
            &format!("project {pid} is archived: it takes no new tasks"),
        );
    }
    if !matches!(project.stage.as_str(), "implementation" | "review") {
        return Response::error(
            409,
            &format!(
                "project {pid} is at {}: tasks can be moved in only once its plan is approved",
                project.stage
            ),
        );
    }
    let moved = match app.plaza.tasks.adopt(id, pid) {
        Ok(moved) => moved,
        Err(refused) => return Response::error(refused.status(), &refused.to_string()),
    };
    for task in &moved {
        if let Err(refused) = app.projects.link_task(pid, task.id()) {
            eprintln!(
                "apid: task {} not linked to project {pid}: {refused}",
                task.id()
            );
        }
    }
    if let Some(project) = app.projects.get(pid) {
        app.bus.publish("project", &project);
    }
    let views: Vec<_> = moved.iter().map(|task| task.view()).collect();
    Response::json(200, &serde_json::json!({ "project": pid, "moved": views }))
}

/// `GET /v1/tasks/{id}`: one task.
fn get_task(app: &App, id: &str) -> Response {
    match app.plaza.tasks.find(id) {
        Some(task) => Response::json(200, &task.view()),
        None => Response::error(404, &format!("no task with id {id}")),
    }
}

/// `POST /v1/tasks/{id}/claim`: binds an open task to a connected agent. The
/// register check is made here; the ledger refuses the claim itself when the
/// task is not open, or not ready (it runs after a task that is not done).
fn claim_task(app: &App, id: &str, body: &[u8]) -> Response {
    let Some(task) = app.plaza.tasks.find(id) else {
        return Response::error(404, &format!("no task with id {id}"));
    };
    let request: ClaimRequest = match parse(body, "claim") {
        Ok(request) => request,
        Err(refused) => return refused,
    };
    let agent = match connected(app, request.agent_cid) {
        Ok(agent) => agent,
        Err(refused) => return refused,
    };
    if let Err(refused) = takes(app, agent.cid, task.project()) {
        return refused;
    }
    match task.claim(&agent) {
        Ok(()) => Response::json(200, &task.view()),
        Err(refused) => Response::error(refused.status(), &refused.to_string()),
    }
}

/// `POST /v1/tasks/{id}/finish`: ends a claimed task as `done` or `failed`.
fn finish_task(app: &App, id: &str, body: &[u8]) -> Response {
    let Some(task) = app.plaza.tasks.find(id) else {
        return Response::error(404, &format!("no task with id {id}"));
    };
    let request: FinishRequest = match parse(body, "finish") {
        Ok(request) => request,
        Err(refused) => return refused,
    };
    if let Err(reason) = request.checked() {
        return Response::error(400, &reason);
    }
    match task.finish(request.outcome, request.note) {
        Ok(()) => Response::json(200, &task.view()),
        Err(refused) => Response::error(409, &refused.to_string()),
    }
}

/// `DELETE /v1/tasks/{id}`: withdraws an open task.
fn withdraw_task(app: &App, id: &str) -> Response {
    match app.plaza.tasks.find(id) {
        Some(task) => match task.withdraw() {
            Ok(()) => Response::json(200, &task.view()),
            Err(refused) => Response::error(409, &refused.to_string()),
        },
        None => Response::error(404, &format!("no task with id {id}")),
    }
}

/// `GET /v1/board?project=`: the board, with presence taken from the roster. A project names one
/// project's posts; `paused` says whether that board's deliveries are held (and `paused_all` whether
/// the whole board is).
fn board_view(app: &App, req: &Request) -> Response {
    let project = req
        .query_param("project")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string);
    if let Some(id) = &project
        && app.projects.get(id).is_none()
    {
        return Response::error(400, &format!("no project {id}"));
    }
    let connected: Vec<u64> = app
        .agents
        .list()
        .iter()
        .filter(|agent| agent.status != Status::Offline)
        .map(|agent| agent.cid)
        .collect();
    let index = ProjectIndex::new(app);
    let view = app
        .plaza
        .social
        .board_for(&connected, social::BOARD_LIMIT, |post| {
            project
                .as_deref()
                .is_none_or(|id| index.of(post).as_deref() == Some(id))
        });
    let pauses = app.settings.get().board_pauses;
    Response::json(
        200,
        &serde_json::json!({
            "posts": view.posts,
            "total": view.total,
            "limit": view.limit,
            "truncated": view.truncated,
            "unread": view.unread,
            "project": project,
            "paused": pauses.holds(project.as_deref()),
            "paused_all": pauses.all,
            "paused_projects": pauses.projects,
        }),
    )
}

/// `POST /v1/board`: a connected agent posts. The author is always an agent;
/// the driver defaults to the caller. The post is on the author's project's
/// board; an agent whose project the owner paused or archived (the project's own pause, which holds
/// its agents), does not post (409). The board's pause (`/v1/board/pause`) is different: it stores the
/// post and only holds the deliveries of its mentions until it resumes (settings::BoardPauses).
fn board_post(app: &App, body: &[u8]) -> Response {
    let request: PostRequest = match parse(body, "post") {
        Ok(request) => request,
        Err(refused) => return refused,
    };
    let mut post = match request.checked() {
        Ok(post) => post,
        Err(reason) => return Response::error(400, &reason),
    };
    if let Some(task) = &post.task
        && app.plaza.tasks.find(task).is_none()
    {
        return Response::error(400, &format!("no task with id {task}"));
    }
    let author = match connected(app, request.agent_cid) {
        Ok(agent) => agent,
        Err(refused) => return refused,
    };
    if let Some(why) = crate::projects::held_cid(app, author.cid) {
        return Response::error(409, &why);
    }
    post.project = crate::projects::project_of(app, author.cid);
    match app.plaza.social.post(&author, post) {
        Ok(post) => Response::json(201, &post),
        Err(refused) => Response::error(refused.status(), refused.message()),
    }
}

/// `POST /v1/board/{id}/comments` `{text}`: the owner comments on a post. The
/// post's author is told through its work queue, unless the owner wrote it.
fn board_comment(app: &App, post: &str, body: &[u8]) -> Response {
    #[derive(Deserialize)]
    struct CommentRequest {
        text: String,
    }
    let request: CommentRequest = match parse(body, "comment") {
        Ok(request) => request,
        Err(refused) => return refused,
    };
    match app.plaza.social.comment(post, None, "owner", &request.text) {
        Ok(comment) => {
            notify_post_author(app, post, None, "the owner", &request.text);
            Response::json(201, &comment)
        }
        Err(refused) => Response::error(refused.status(), refused.message()),
    }
}

/// Tells a post's author that someone commented on it, through the author's
/// work queue. The author is not told of its own comments.
pub(crate) fn notify_post_author(
    app: &App,
    post: &str,
    commenter: Option<u64>,
    who: &str,
    text: &str,
) {
    let Some(found) = app
        .plaza
        .social
        .board(&[], 200)
        .posts
        .into_iter()
        .find(|p| p.id == post)
    else {
        return;
    };
    // The owner's own posts have no agent to tell, and a paused board is not nudged: the comment is
    // stored on the post, where the author sees it when it reads the board (no replay on resume).
    if found.owner
        || app
            .settings
            .get()
            .board_pauses
            .holds(ProjectIndex::new(app).of(&found).as_deref())
    {
        return;
    }
    let (author_cid, subject) = (found.agent_cid, found.subject);
    // An author whose project is paused or over sees the comment when it works again.
    if Some(author_cid) == commenter || crate::projects::held_cid(app, author_cid).is_some() {
        return;
    }
    app.work.enqueue(
        app,
        author_cid,
        WorkKind::Message,
        format!("board-{post}"),
        format!(
            "[plaza board] {who} commented on your post {post} \"{subject}\": {text}\nIf it needs an answer, reply on the thread with plaza_board_comment."
        ),
        Origin::default(),
    );
}

/// `POST /v1/board/read`: a connected agent marks the board read up to a point.
fn board_read(app: &App, body: &[u8]) -> Response {
    let request: ReadRequest = match parse(body, "read") {
        Ok(request) => request,
        Err(refused) => return refused,
    };
    let reader = match connected(app, request.agent_cid) {
        Ok(agent) => agent,
        Err(refused) => return refused,
    };
    match app.plaza.social.read(&reader, &request.up_to) {
        Ok(read) => Response::json(200, &read),
        Err(refused) => Response::error(refused.status(), refused.message()),
    }
}

/// A query-string value, percent-decoded (`+` is a space). A malformed escape
/// is kept as it is.
pub(crate) fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'+' {
            out.push(b' ');
        } else if byte == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3])
                .ok()
                .and_then(|pair| u8::from_str_radix(pair, 16).ok());
            match hex {
                Some(value) => {
                    out.push(value);
                    index += 2;
                }
                None => out.push(byte),
            }
        } else {
            out.push(byte);
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Applies the owner's retention to the board: the count cap, then the age limit. Runs on the
/// scheduler's tick, so a change to the retention takes effect within a tick.
pub fn retain(app: &App) {
    app.plaza
        .social
        .prune(app.settings.get().retention.policy());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignee_is_optional_and_must_be_a_number() {
        assert_eq!(
            parse::<Assignee>(b"{}", "create")
                .ok()
                .and_then(|a| a.assignee),
            None
        );
        assert_eq!(
            parse::<Assignee>(br#"{"assignee": 7}"#, "create")
                .ok()
                .and_then(|a| a.assignee),
            Some(7)
        );
        assert!(parse::<Assignee>(br#"{"assignee": "seven"}"#, "create").is_err());
    }

    #[test]
    fn a_create_body_may_name_its_links_and_its_project() {
        let request: CreateTask = parse(
            br#"{"goal": "work", "after": ["t1", "t2"], "project": "pr1"}"#,
            "create",
        )
        .expect("a create body with links");
        assert_eq!(request.after, vec!["t1", "t2"]);
        assert_eq!(request.project.as_deref(), Some("pr1"));
        // A body from before links names neither.
        let request: CreateTask =
            parse(br#"{"goal": "work"}"#, "create").expect("a create body without");
        assert!(request.after.is_empty());
        assert_eq!(request.project, None);
        assert!(parse::<CreateTask>(br#"{"goal": "work", "after": "t1"}"#, "create").is_err());
    }
}

#[cfg(test)]
mod query_decode_tests {
    use super::percent_decode;

    #[test]
    fn a_search_value_decodes_escapes_and_plus_and_keeps_malformed_ones() {
        assert_eq!(percent_decode("budget+report"), "budget report");
        assert_eq!(percent_decode("a%20b%3Fc"), "a b?c");
        assert_eq!(percent_decode("100%"), "100%", "a trailing % is kept");
        assert_eq!(percent_decode("%zz"), "%zz", "a non-hex escape is kept");
    }
}
