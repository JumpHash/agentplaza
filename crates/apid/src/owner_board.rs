//! The owner's side of the plaza board (`docs/design/workspace.md`, section 2): the owner posts to
//! the board, for the whole board or one project, with @mentions and `@all`; pauses and resumes the
//! deliveries of a project's board or of the whole board; and exports a thread as Markdown or JSON.
//!
//! Routes: `POST /v1/board/posts`, `POST /v1/board/pause`, `GET /v1/board/export`. The board list
//! (`GET /v1/board?project=`) lives in `plaza.rs` and reads the same [`ProjectIndex`].
//!
//! Owner actions are recorded on the bus like the other owner actions on the board (a `board_paused`
//! event for a pause, the `board` post frame for a post), and an export is a read.

use std::collections::{BTreeSet, HashMap};

use apz_http::{Request, Response, RouteDoc, Router};
use apz_plaza::social::{OwnerPostRequest, PostView};
use apz_proto::wire::WorkKind;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::plaza::parse;
use crate::projects::Project;
use crate::scheduler::Origin;
use crate::state::App;

/// The owner's mention that reaches everyone on the post's project (every agent, without one).
const EVERYONE: &str = "all";

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .post("/v1/board/posts", |app, req, _| owner_post(app, &req.body))
        .doc(
            RouteDoc::new("Post to the board as the owner")
                .status(201)
                .errors(&[400]),
        )
        .post("/v1/board/pause", |app, req, _| pause(app, &req.body))
        .doc(RouteDoc::new("Pause or resume a board's deliveries").errors(&[400, 500]))
        .get("/v1/board/export", |app, req, _| export(app, req))
        .doc(RouteDoc::new("Export a board thread as Markdown or JSON").errors(&[400]))
}

/// Which project each post is for: the post's own project, or else the project whose tasks it names.
pub struct ProjectIndex(HashMap<String, String>);

impl ProjectIndex {
    /// The index over the projects the hub holds now: each task a project names maps to it.
    pub fn new(app: &App) -> Self {
        Self::from_tasks(app.projects.list().into_iter().flat_map(|project| {
            let id = project.id.clone();
            project
                .tasks
                .into_iter()
                .map(move |task| (task, id.clone()))
        }))
    }

    /// The index over `(task, project)` pairs.
    pub fn from_tasks(pairs: impl IntoIterator<Item = (String, String)>) -> Self {
        Self(pairs.into_iter().collect())
    }

    /// The project `post` is for, or `None` for the whole board.
    pub fn of(&self, post: &PostView) -> Option<String> {
        post.project.clone().or_else(|| {
            post.task
                .as_ref()
                .and_then(|task| self.0.get(task).cloned())
        })
    }
}

/// `POST /v1/board/posts` `{subject?, body, project?, mentions?}`: the owner posts. A post for a
/// project must name an existing one. Each agent the post mentions is sent a message through its
/// work queue, unless the post's board is paused, or the agent does not work now (it is on the bench,
/// or its project is paused): then the post is stored only, and the agent sees it through its unread
/// cursor.
fn owner_post(app: &App, body: &[u8]) -> Response {
    let request: OwnerPostRequest = match parse(body, "board post") {
        Ok(request) => request,
        Err(refused) => return refused,
    };
    let checked = match request.checked() {
        Ok(checked) => checked,
        Err(reason) => return Response::error(400, &reason),
    };
    let project = match &checked.project {
        Some(id) => match app.projects.get(id) {
            Some(project) => Some(project),
            None => return Response::error(400, &format!("no project {id}")),
        },
        None => None,
    };
    let everyone: Vec<u64> = app.agents.select(|agent| Some(agent.cid));
    let roster: Vec<(u64, String, String)> = app
        .agents
        .list()
        .into_iter()
        .map(|agent| (agent.cid, agent.label(), agent.role))
        .collect();
    let members = project
        .as_ref()
        .map(|project| project_members(app, project));
    let reached = match reach(&checked.mentions, members.as_deref(), &everyone, &roster) {
        Ok(reached) => reached,
        Err(reason) => return Response::error(400, &reason),
    };
    let post = app.plaza.social.post_as_owner(checked);
    if app
        .settings
        .get()
        .board_pauses
        .holds(post.project.as_deref())
    {
        return Response::json(201, &post);
    }
    let scope = project
        .as_ref()
        .map(|p| format!(" on project {} ({})", p.id, p.title))
        .unwrap_or_default();
    let prompt = format!(
        "[plaza board] The owner posted {} \"{}\"{scope} and mentioned you:\n{}\nIf it needs an answer, reply on the thread with plaza_board_comment.",
        post.id, post.subject, post.body
    );
    for cid in reached
        .into_iter()
        .filter(|&cid| crate::membership::held_cid(app, cid).is_none())
    {
        app.work.enqueue(
            app,
            cid,
            WorkKind::Message,
            format!("board-{}", post.id),
            prompt.clone(),
            Origin::default(),
        );
    }
    Response::json(201, &post)
}

/// The agents on a project: its orchestrator (an owner-opened project has none) and its members.
fn project_members(app: &App, project: &Project) -> Vec<u64> {
    let lead = (project.lead_cid != 0).then_some(project.lead_cid);
    dedup(
        lead.into_iter()
            .chain(app.members.members(&project.id))
            .collect(),
    )
}

/// The guest CIDs a post's mentions reach. `all` is every member of the post's project (`members`,
/// `Some`), or every agent when the post has no project (`None`, with `everyone`). A role is every
/// agent that holds it; a mention nobody matches is refused, so a typo is not a silent non-delivery.
/// `roster` is `(cid, label, role)` for each agent.
fn reach(
    mentions: &[String],
    members: Option<&[u64]>,
    everyone: &[u64],
    roster: &[(u64, String, String)],
) -> Result<Vec<u64>, String> {
    let mut cids = Vec::new();
    for name in mentions {
        if name.eq_ignore_ascii_case(EVERYONE) {
            cids.extend_from_slice(members.unwrap_or(everyone));
            continue;
        }
        let matched: Vec<u64> = roster
            .iter()
            .filter(|(_, agent, role)| {
                agent.eq_ignore_ascii_case(name) || role.eq_ignore_ascii_case(name)
            })
            .map(|(cid, _, _)| *cid)
            .collect();
        if matched.is_empty() {
            return Err(format!(
                "no agent has the role {name}: mention an agent's role, or {EVERYONE}"
            ));
        }
        cids.extend(matched);
    }
    Ok(dedup(cids))
}

/// The values in ascending order, each once.
fn dedup(cids: Vec<u64>) -> Vec<u64> {
    cids.into_iter()
        .collect::<BTreeSet<u64>>()
        .into_iter()
        .collect()
}

/// `POST /v1/board/pause` `{project?, on}`: pauses (`on: true`) or resumes the deliveries of a
/// project's board, or of the whole board when no project is named. Posts are stored either way.
fn pause(app: &App, body: &[u8]) -> Response {
    let request: PauseBody = match parse(body, "pause") {
        Ok(request) => request,
        Err(refused) => return refused,
    };
    let project = request
        .project
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty());
    if let Some(id) = &project
        && app.projects.get(id).is_none()
    {
        return Response::error(400, &format!("no project {id}"));
    }
    let (changed, pauses) = match app.settings.set_board_pause(project.as_deref(), request.on) {
        Ok(saved) => saved,
        Err(reason) => return Response::error(500, &format!("the pause is not saved: {reason}")),
    };
    app.bus.publish(
        "board_paused",
        &json!({ "project": project, "on": request.on }),
    );
    Response::json(
        200,
        &json!({
            "project": project,
            "on": request.on,
            "changed": changed,
            "paused": pauses.holds(project.as_deref()),
            "paused_all": pauses.all,
            "paused_projects": pauses.projects,
        }),
    )
}

#[derive(Deserialize)]
struct PauseBody {
    #[serde(default)]
    project: Option<String>,
    on: bool,
}

/// `GET /v1/board/export?project=&format=md|json`: the whole thread (every post with its comments)
/// for a project, or for the whole board, oldest first, as a download.
fn export(app: &App, req: &Request) -> Response {
    let format = req.query_param("format").unwrap_or("md");
    if format != "md" && format != "json" {
        return Response::error(400, &format!("format must be md or json, not {format}"));
    }
    let project = req
        .query_param("project")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string);
    let title = match &project {
        Some(id) => match app.projects.get(id) {
            Some(project) => format!("{} ({})", project.title, project.id),
            None => return Response::error(400, &format!("no project {id}")),
        },
        None => "whole board".to_string(),
    };
    let index = ProjectIndex::new(app);
    let posts = app.plaza.social.posts_matching(|post| {
        project
            .as_deref()
            .is_none_or(|id| index.of(post).as_deref() == Some(id))
    });
    let now = apz_plaza::now_ms();
    let scope = project.clone().unwrap_or_else(|| "all".to_string());
    if format == "json" {
        let rows: Vec<Value> = posts
            .iter()
            .map(|post| {
                let mut row = serde_json::to_value(post).unwrap_or(Value::Null);
                row["project"] = json!(index.of(post));
                row
            })
            .collect();
        let body = json!({ "project": project, "exported_at_ms": now, "total": rows.len(), "posts": rows });
        return Response::json(200, &body)
            .with_header("Content-Disposition", &disposition(&scope, "json"));
    }
    let text = markdown(&title, &posts, |post| index.of(post), now);
    Response::new(200, "text/markdown; charset=utf-8", text)
        .with_header("Content-Disposition", &disposition(&scope, "md"))
}

/// The download's file name: the project's id (or `all`), with only letters, digits and dashes.
fn disposition(scope: &str, ext: &str) -> String {
    let safe: String = scope
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    format!("attachment; filename=\"agentplaza-board-{safe}.{ext}\"")
}

/// A thread as Markdown: a heading, then each post with its project, mentions and comments.
fn markdown(
    title: &str,
    posts: &[PostView],
    project_of: impl Fn(&PostView) -> Option<String>,
    now: u64,
) -> String {
    let mut out = format!(
        "# Plaza board: {title}\n\nExported {} · {} posts\n",
        iso(now),
        posts.len()
    );
    for post in posts {
        let author = if post.owner {
            "Owner"
        } else {
            post.role.as_str()
        };
        out.push_str(&format!(
            "\n## {} · {} · {}\n\n",
            post.id,
            author,
            iso(post.created_at)
        ));
        if !post.subject.is_empty() {
            out.push_str(&format!("**{}**\n\n", post.subject));
        }
        let mut meta = Vec::new();
        if let Some(project) = project_of(post) {
            meta.push(format!("Project {project}"));
        }
        if !post.mentions.is_empty() {
            meta.push(format!(
                "Mentions {}",
                post.mentions
                    .iter()
                    .map(|m| format!("@{m}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if let Some(task) = &post.task {
            meta.push(format!("Task {task}"));
        }
        if !meta.is_empty() {
            out.push_str(&format!("{}\n\n", meta.join(" · ")));
        }
        out.push_str(&format!("{}\n", post.body));
        if !post.comments.is_empty() {
            out.push_str("\n### Comments\n");
            for comment in &post.comments {
                out.push_str(&format!(
                    "\n- **{}** · {}\n",
                    comment.author,
                    iso(comment.created_at)
                ));
                for line in comment.body.lines() {
                    out.push_str(&format!("  {line}\n"));
                }
            }
        }
    }
    out
}

/// Milliseconds since the Unix epoch as `YYYY-MM-DD HH:MM UTC`.
fn iso(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Days since the epoch to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rem / 3600,
        rem % 3600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn roster() -> Vec<(u64, String, String)> {
        vec![
            (3, "Ada".to_string(), "builder".to_string()),
            (4, "Lin".to_string(), "builder".to_string()),
            (5, "Rex".to_string(), "reviewer".to_string()),
        ]
    }

    #[test]
    fn a_mention_reaches_its_agents_and_all_reaches_the_project_or_everyone() {
        // A mention reaches the named agent, or every agent with the role; an unknown name is refused.
        {
            assert_eq!(
                reach(&names(&["ada"]), None, &[], &roster()),
                Ok(vec![3]),
                "names match without case"
            );
            assert_eq!(
                reach(&names(&["builder"]), None, &[], &roster()),
                Ok(vec![3, 4]),
                "a role reaches each holder"
            );
            assert_eq!(
                reach(&names(&["Ada", "builder"]), None, &[], &roster()),
                Ok(vec![3, 4]),
                "repeats reach once"
            );
            assert!(
                reach(&names(&["nobody"]), None, &[], &roster()).is_err(),
                "an unknown name is refused"
            );
        }
        // All is the project's members, or every agent without one; an empty project reaches nobody.
        {
            let everyone = [3, 4, 5];
            assert_eq!(
                reach(&names(&["all"]), Some(&[4]), &everyone, &roster()),
                Ok(vec![4]),
                "all on a project is its members"
            );
            assert_eq!(
                reach(&names(&["ALL"]), None, &everyone, &roster()),
                Ok(vec![3, 4, 5]),
                "no project: every agent"
            );
            assert_eq!(
                reach(&names(&["all", "Rex"]), Some(&[4]), &everyone, &roster()),
                Ok(vec![4, 5])
            );
            assert_eq!(
                reach(&names(&["all"]), Some(&[]), &everyone, &roster()),
                Ok(vec![]),
                "an empty project reaches nobody, not everyone"
            );
        }
    }

    #[test]
    fn a_project_index_takes_a_post_from_its_own_project_or_its_task() {
        let index = ProjectIndex::from_tasks([("t1".to_string(), "pr1".to_string())]);
        let post = |project: Option<&str>, task: Option<&str>| PostView {
            id: "p1".into(),
            agent_cid: 3,
            role: "builder".into(),
            subject: String::new(),
            body: String::new(),
            task: task.map(str::to_string),
            posted_by: "driver".into(),
            created_at: 0,
            comments: Vec::new(),
            attachments: Vec::new(),
            owner: false,
            project: project.map(str::to_string),
            mentions: Vec::new(),
        };
        assert_eq!(index.of(&post(None, Some("t1"))).as_deref(), Some("pr1"));
        assert_eq!(
            index.of(&post(Some("pr2"), Some("t1"))).as_deref(),
            Some("pr2"),
            "its own project wins"
        );
        assert_eq!(index.of(&post(None, Some("t9"))), None);
        assert_eq!(index.of(&post(None, None)), None);
    }

    #[test]
    fn a_pause_names_a_project_or_the_whole_board() {
        let body: PauseBody = serde_json::from_str(r#"{"on": true}"#).unwrap();
        assert_eq!((body.project, body.on), (None, true));
        let body: PauseBody = serde_json::from_str(r#"{"project": "pr1", "on": false}"#).unwrap();
        assert_eq!((body.project.as_deref(), body.on), (Some("pr1"), false));
        assert!(
            serde_json::from_str::<PauseBody>(r#"{"project": "pr1"}"#).is_err(),
            "on is required"
        );
    }

    #[test]
    fn the_download_name_is_safe() {
        assert_eq!(
            disposition("pr12", "md"),
            "attachment; filename=\"agentplaza-board-pr12.md\""
        );
        assert_eq!(
            disposition("../x y", "json"),
            "attachment; filename=\"agentplaza-board-xy.json\""
        );
    }

    #[test]
    fn a_thread_renders_as_markdown_with_mentions_and_comments() {
        let post = PostView {
            id: "p2".into(),
            agent_cid: 0,
            role: "owner".into(),
            subject: "Release".into(),
            body: "Ship it.\nToday.".into(),
            task: None,
            posted_by: "owner".into(),
            created_at: 1_791_970_200_000,
            comments: vec![apz_plaza::social::CommentView {
                id: "c1".into(),
                post: "p2".into(),
                agent_cid: Some(3),
                author: "builder".into(),
                body: "On it".into(),
                created_at: 1_791_970_260_000,
                attachments: Vec::new(),
            }],
            attachments: Vec::new(),
            owner: true,
            project: Some("pr1".into()),
            mentions: vec!["builder".into(), "all".into()],
        };
        let text = markdown(
            "Alpha (pr1)",
            std::slice::from_ref(&post),
            |p| p.project.clone(),
            1_791_970_300_000,
        );
        assert!(text.starts_with("# Plaza board: Alpha (pr1)\n"));
        assert!(text.contains("## p2 · Owner · 2026-10-14 09:30 UTC"));
        assert!(text.contains("**Release**"));
        assert!(text.contains("Project pr1 · Mentions @builder, @all"));
        assert!(text.contains("Ship it.\nToday."));
        assert!(text.contains("- **builder** · 2026-10-14 09:31 UTC\n  On it\n"));
    }
}
