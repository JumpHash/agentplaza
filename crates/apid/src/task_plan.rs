//! A plan's task list as a graph: the order a plan gives its tasks, checked when the plan is
//! attached and kept when the orchestrator posts it.
//!
//! The planner's task list is the project's `tasks` document (for code, the OpenSpec change's
//! `tasks.md`). A task is a checklist line keyed by its number, `- [ ] 1.3 Write the release
//! notes`, and it may end with the tasks it runs after: `(after: 1.1, 1.2)`. A task with no such
//! note runs as soon as the plan is posted. A fenced `json` block holding `{"tasks": [{"key",
//! "goal", "after"}]}` (or the array alone) is read instead when the document has one.
//!
//! Attaching the document checks the list ([`check`]): every key once, every link naming a task of
//! the list, and no task waiting on itself through others. The orchestrator posts the approved
//! list whole with `plaza_tasks_post` ([`post`]): one batch the ledger creates entirely or not at
//! all, each task running after the tasks its links name.

use std::collections::HashMap;

use apz_plaza::tasks::{BatchTask, CreateTask, TaskLedger, resolve_budget};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::projects::{Project, Store};
use crate::state::App;

/// The most tasks one list may hold.
pub const MAX_PLANNED: usize = 64;

/// The most bytes of a `tasks` document that are read to check it.
const MAX_DOC_BYTES: usize = 256 * 1024;

/// One task of a plan's list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Planned {
    /// The task's key in the list (`1.3`), which other tasks' `after` names.
    pub key: String,
    /// What the task is for.
    pub goal: String,
    /// How the work is judged complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<String>,
    /// The role it is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Its budget, in credits (the default when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<u64>,
    /// The keys of the tasks of the list it runs after (or, when posting, the ids of tasks that
    /// exist already).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
}

/// The task list in a `tasks` document: the fenced `json` block when it has one, else its
/// checklist lines; empty when it has neither. A `json` block that does not read as a task list
/// is an error.
pub fn parse(text: &str) -> Result<Vec<Planned>, String> {
    if let Some(block) = json_block(text) {
        let value: Value = serde_json::from_str(block)
            .map_err(|err| format!("the json block does not parse: {err}"))?;
        let list = match value {
            Value::Object(mut object) => object.remove("tasks").unwrap_or(Value::Null),
            other => other,
        };
        return serde_json::from_value(list).map_err(|err| {
            format!("the json block is not a task list ({{\"tasks\": [{{\"key\", \"goal\", \"after\"}}]}}): {err}")
        });
    }
    Ok(text.lines().filter_map(checklist_task).collect())
}

/// The first fenced block marked `json`, without its fences.
fn json_block(text: &str) -> Option<&str> {
    let start = text.find("```json")?;
    let body = &text[start + "```json".len()..];
    let body = body
        .strip_prefix('\n')
        .or_else(|| body.strip_prefix("\r\n"))?;
    let end = body.find("```")?;
    Some(&body[..end])
}

/// A checklist line as a task: `- [ ] 1.3 Goal (after: 1.1, 1.2)` (also `*` and `[x]`). Any other
/// line is not one.
fn checklist_task(line: &str) -> Option<Planned> {
    let rest = line.trim_start().strip_prefix(['-', '*'])?.trim_start();
    let rest = rest
        .strip_prefix("[ ]")
        .or_else(|| rest.strip_prefix("[x]"))
        .or_else(|| rest.strip_prefix("[X]"))?
        .trim_start();
    let (key, goal) = rest.split_once(char::is_whitespace)?;
    let key = key.trim_end_matches('.');
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    let goal = goal.trim();
    // `(after: …)` closes the line, in any case; ASCII lowercasing keeps every byte offset.
    let note = goal
        .strip_suffix(')')
        .and_then(|head| head.to_ascii_lowercase().rfind("(after:"));
    let (goal, after) = match note {
        Some(at) => (
            goal[..at].trim_end(),
            goal[at + "(after:".len()..goal.len() - 1]
                .split(',')
                .map(|link| link.trim().to_string())
                .filter(|link| !link.is_empty())
                .collect(),
        ),
        None => (goal, Vec::new()),
    };
    Some(Planned {
        key: key.to_string(),
        goal: goal.to_string(),
        acceptance: None,
        role: None,
        budget: None,
        after,
    })
}

/// Checks `tasks` as a graph and returns their places in an order where each task comes after
/// every task of the list it runs after, keeping the list's own order where the links allow. A
/// link that names no key of the list may name a task that exists already, when `exists` says so.
pub fn check(tasks: &[Planned], exists: impl Fn(&str) -> bool) -> Result<Vec<usize>, String> {
    if tasks.len() > MAX_PLANNED {
        return Err(format!(
            "the list holds {} tasks, more than {MAX_PLANNED}",
            tasks.len()
        ));
    }
    let mut index: HashMap<&str, usize> = HashMap::new();
    for (at, task) in tasks.iter().enumerate() {
        let key = task.key.trim();
        if key.is_empty() {
            return Err(format!("task {} of the list has no key", at + 1));
        }
        if task.goal.trim().is_empty() {
            return Err(format!("task {key} has no goal"));
        }
        if index.insert(key, at).is_some() {
            return Err(format!("the key {key} is used twice"));
        }
    }
    let mut links: Vec<Vec<usize>> = Vec::with_capacity(tasks.len());
    for task in tasks {
        let key = task.key.trim();
        let mut own = Vec::new();
        let mut named: Vec<&str> = Vec::new();
        for link in &task.after {
            let link = link.trim();
            if named.contains(&link) {
                return Err(format!("{key} names {link} twice"));
            }
            named.push(link);
            match index.get(link) {
                Some(&at) => own.push(at),
                None if exists(link) => {}
                None => {
                    return Err(format!(
                        "{key} runs after {link}, which is not a task of the list"
                    ));
                }
            }
        }
        links.push(own);
    }
    // Each pass takes the first task, in the list's order, whose links are all taken.
    let mut taken = vec![false; tasks.len()];
    let mut order = Vec::with_capacity(tasks.len());
    while let Some(next) =
        (0..tasks.len()).find(|&at| !taken[at] && links[at].iter().all(|&from| taken[from]))
    {
        taken[next] = true;
        order.push(next);
    }
    if order.len() < tasks.len() {
        return Err(format!(
            "the links close a cycle: {}",
            describe_cycle(tasks, &links, &taken)
        ));
    }
    Ok(order)
}

/// A cycle among the tasks not `taken`, as `a runs after b, which runs after a`. Every task left
/// waits on another one left, so following those links from any of them comes back round.
fn describe_cycle(tasks: &[Planned], links: &[Vec<usize>], taken: &[bool]) -> String {
    let Some(start) = (0..tasks.len()).find(|&at| !taken[at]) else {
        return String::new();
    };
    let mut path = vec![start];
    loop {
        let current = *path.last().unwrap_or(&start);
        let Some(&next) = links[current].iter().find(|&&from| !taken[from]) else {
            return String::new();
        };
        if let Some(begin) = path.iter().position(|&at| at == next) {
            let mut cycle: Vec<&str> = path[begin..]
                .iter()
                .map(|&at| tasks[at].key.trim())
                .collect();
            cycle.push(tasks[next].key.trim());
            let (first, rest) = cycle.split_first().unwrap_or((&"", &[]));
            return format!("{first} runs after {}", rest.join(", which runs after "));
        }
        path.push(next);
    }
}

/// What a check of a `tasks` document found, for the attach's answer.
pub fn summary(tasks: &[Planned]) -> String {
    let linked = tasks.iter().filter(|task| !task.after.is_empty()).count();
    format!(
        "The tasks list holds {} tasks; {linked} of them run after others",
        tasks.len()
    )
}

/// The text of a project's document: a shared file from the plaza's store, or a file in the
/// project's repo on the plaza's git host (at `commit`, else the default branch).
pub fn read_doc(
    app: &App,
    project: &Project,
    file: Option<&str>,
    repo_path: Option<&str>,
    commit: Option<&str>,
) -> Result<String, String> {
    if let Some(file) = file {
        let meta = app
            .deliverables
            .meta(file)
            .ok_or_else(|| format!("no file {file}"))?;
        let bytes = app.deliverables.read_blob(&meta)?;
        return text_of(&bytes);
    }
    let path = repo_path.ok_or("the document names no file or repo path")?;
    let repo = project
        .repo
        .as_deref()
        .ok_or("the project has no repo in Weft")?;
    let repo = repo.strip_prefix("agentplaza/").unwrap_or(repo);
    // The document on main, or at the revision (or landed change) the plan names.
    let rev = match commit {
        Some(c) if c.starts_with("w-") && !c.contains('.') => format!("{c}.l"),
        Some(c) => c.to_string(),
        None => app
            .weft
            .get(&format!("/repos/{}", weft::server::percent_encode(repo)))?["refs"]["main"]
            .as_str()
            .unwrap_or("root")
            .to_string(),
    };
    let client = app.weft.client("owner")?;
    let bytes = client
        .get_bytes(&format!(
            "/revisions/{rev}/file?path={}",
            weft::server::percent_encode(path.trim_start_matches('/'))
        ))
        .map_err(|e| format!("Weft has no {path} in {repo} at {rev}: {e}"))?;
    text_of(&bytes)
}

/// The bytes as text, refused when they are not UTF-8 or too long to be a task list.
fn text_of(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() > MAX_DOC_BYTES {
        return Err(format!(
            "the document is {} bytes, more than a task list ({MAX_DOC_BYTES})",
            bytes.len()
        ));
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| "the document is not UTF-8 text".to_string())
}

/// One task of a posted list: its key, the id the ledger gave it, the ids it runs after, and
/// whether it can be claimed now.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Posted {
    pub key: String,
    pub id: String,
    pub after: Vec<String>,
    pub ready: bool,
}

/// Posts project `pid`'s task list whole. The project's plan must be approved (implementation or
/// review); the list is checked ([`check`], where a link may also name a task that exists); then
/// the ledger creates every task or none, in an order where each comes after the tasks it runs
/// after, and each is linked to the project. A task that names no budget gets `default_budget`
/// (the project's), else the plaza's default. Returns the tasks in the list's order.
pub fn post(
    projects: &Store,
    ledger: &TaskLedger,
    pid: &str,
    tasks: &[Planned],
    default_budget: Option<u64>,
) -> Result<Vec<Posted>, String> {
    let project = projects
        .get(pid)
        .ok_or_else(|| format!("no project {pid}"))?;
    // In maintenance a delivered project takes the work the owner picked.
    let maintained = project.in_maintenance() && !project.archived;
    if !maintained && !matches!(project.stage.as_str(), "implementation" | "review") {
        return Err(format!(
            "project {pid} is at {}: tasks can be posted only once its plan is approved",
            project.stage
        ));
    }
    if tasks.is_empty() {
        return Err("the list has no tasks".to_string());
    }
    let order = check(tasks, |id| ledger.find(id).is_some())?;
    let index: HashMap<&str, usize> = tasks
        .iter()
        .enumerate()
        .map(|(at, task)| (task.key.trim(), at))
        .collect();
    // Where each task of the list stands in the batch.
    let mut place = vec![0; tasks.len()];
    for (in_batch, &at) in order.iter().enumerate() {
        place[at] = in_batch;
    }
    let mut batch = Vec::with_capacity(tasks.len());
    for &at in &order {
        let task = &tasks[at];
        let budget = resolve_budget(task.budget.or(default_budget))
            .map_err(|refusal| format!("task {}: {refusal}", task.key.trim()))?;
        let (own, existing): (Vec<&str>, Vec<&str>) = task
            .after
            .iter()
            .map(|link| link.trim())
            .partition(|link| index.contains_key(link));
        batch.push(BatchTask {
            request: CreateTask {
                goal: task.goal.trim().to_string(),
                acceptance: task.acceptance.clone(),
                budget: Some(budget),
                role: task.role.clone(),
                after: existing.into_iter().map(str::to_string).collect(),
                project: Some(pid.to_string()),
            },
            after_batch: own.into_iter().map(|key| place[index[key]]).collect(),
        });
    }
    let created = ledger.create_all(&batch).map_err(|refused| {
        // The ledger counts the batch from 1 in posting order; the planner knows its keys.
        let text = refused.to_string();
        order
            .iter()
            .enumerate()
            .find_map(|(in_batch, &at)| {
                text.strip_prefix(&format!("task {} of the batch: ", in_batch + 1))
                    .map(|reason| format!("task {}: {reason}", tasks[at].key.trim()))
            })
            .unwrap_or(text)
    })?;
    for task in &created {
        projects.link_task(pid, task.id())?;
    }
    let mut posted: Vec<(usize, Posted)> = order
        .iter()
        .zip(&created)
        .map(|(&at, task)| {
            let view = task.view();
            (
                at,
                Posted {
                    key: tasks[at].key.trim().to_string(),
                    id: task.id().to_string(),
                    after: view.after,
                    ready: view.ready,
                },
            )
        })
        .collect();
    posted.sort_by_key(|(at, _)| *at);
    Ok(posted.into_iter().map(|(_, posted)| posted).collect())
}

#[cfg(test)]
mod tests {
    use apz_plaza::ConnectedAgent;
    use apz_plaza::events::EventBus;
    use apz_plaza::tasks::FinishOutcome;

    use super::*;
    use crate::projects::{Actor, Doc};

    fn planned(key: &str, goal: &str, after: &[&str]) -> Planned {
        Planned {
            key: key.to_string(),
            goal: goal.to_string(),
            acceptance: None,
            role: None,
            budget: None,
            after: after.iter().map(|key| key.to_string()).collect(),
        }
    }

    const TASKS_MD: &str = "## 1. Release 0.9\n\n\
        - [ ] 1.1 Patch the flaky scheduler test\n\
        - [ ] 1.2 Land the sanction fix\n\
        - [x] 1.3 Write the release notes (After: 1.1)\n\
        * [ ] 1.4 QA on staging (after: 1.2, 1.3)\n\
        \n\
        Notes: 1.4 waits for both fixes.\n\
        - not a task\n";

    #[test]
    fn a_checklist_is_read_as_keyed_tasks_with_their_links() {
        let tasks = parse(TASKS_MD).expect("a list");
        assert_eq!(
            tasks,
            vec![
                planned("1.1", "Patch the flaky scheduler test", &[]),
                planned("1.2", "Land the sanction fix", &[]),
                planned("1.3", "Write the release notes", &["1.1"]),
                planned("1.4", "QA on staging", &["1.2", "1.3"]),
            ]
        );
        assert_eq!(check(&tasks, |_| false), Ok(vec![0, 1, 2, 3]));
        assert_eq!(
            summary(&tasks),
            "The tasks list holds 4 tasks; 2 of them run after others"
        );
        assert!(
            parse("# Tasks\n\nWe will build it.\n")
                .expect("prose")
                .is_empty()
        );
    }

    #[test]
    fn a_json_block_is_read_instead_of_the_checklist() {
        let text = "# Tasks\n\n- [ ] 9.9 ignored\n\n```json\n{\"tasks\": [\
            {\"key\": \"design\", \"goal\": \"Design it\", \"role\": \"architect\"},\
            {\"key\": \"build\", \"goal\": \"Build it\", \"after\": [\"design\"], \"budget\": 3000}\
            ]}\n```\n";
        let tasks = parse(text).expect("a list");
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[1].after, vec!["design"]);
        assert_eq!(tasks[1].budget, Some(3000));
        assert_eq!(tasks[0].role.as_deref(), Some("architect"));
        let bare = parse("```json\n[{\"key\": \"a\", \"goal\": \"A\"}]\n```").expect("an array");
        assert_eq!(bare, vec![planned("a", "A", &[])]);
        assert!(
            parse("```json\n{\"tasks\": 3}\n```")
                .unwrap_err()
                .contains("not a task list")
        );
        assert!(
            parse("```json\nnot json\n```")
                .unwrap_err()
                .contains("does not parse")
        );
    }

    #[test]
    fn a_list_that_does_not_hold_together_is_named() {
        let refused = |tasks: &[Planned]| check(tasks, |_| false).expect_err("refused");
        assert_eq!(
            refused(&[planned("1.1", "a", &[]), planned("1.1", "b", &[])]),
            "the key 1.1 is used twice"
        );
        assert_eq!(
            refused(&[planned("1.1", "a", &["1.9"])]),
            "1.1 runs after 1.9, which is not a task of the list"
        );
        assert_eq!(
            refused(&[
                planned("1.1", "a", &[]),
                planned("1.2", "b", &["1.1", "1.1"])
            ]),
            "1.2 names 1.1 twice"
        );
        assert_eq!(
            refused(&[planned("1.1", "  ", &[])]),
            "task 1.1 has no goal"
        );
        assert_eq!(
            refused(&[
                planned("1.1", "a", &["1.3"]),
                planned("1.2", "b", &["1.1"]),
                planned("1.3", "c", &["1.2"]),
            ]),
            "the links close a cycle: 1.1 runs after 1.3, which runs after 1.2, which runs after 1.1"
        );
        assert_eq!(
            refused(&[planned("1.1", "a", &["1.1"])]),
            "the links close a cycle: 1.1 runs after 1.1"
        );
        let many: Vec<Planned> = (0..=MAX_PLANNED)
            .map(|n| planned(&n.to_string(), "x", &[]))
            .collect();
        assert!(refused(&many).contains("more than 64"));
    }

    #[test]
    fn the_order_keeps_the_list_where_the_links_allow() {
        // Listed release-first, the release still comes after what it runs after.
        let tasks = [
            planned("ship", "ship", &["build", "docs"]),
            planned("build", "build", &["design"]),
            planned("docs", "docs", &[]),
            planned("design", "design", &[]),
        ];
        let order = check(&tasks, |_| false).expect("a graph");
        let keys: Vec<&str> = order.iter().map(|&at| tasks[at].key.as_str()).collect();
        assert_eq!(keys, vec!["docs", "design", "build", "ship"]);
        // A link that is no key of the list may name a task that exists.
        let extra = [planned("fix", "fix", &["t12"])];
        assert!(check(&extra, |_| false).is_err());
        assert_eq!(check(&extra, |id| id == "t12"), Ok(vec![0]));
    }

    #[test]
    fn a_repo_path_is_encoded_for_weft() {
        assert_eq!(
            weft::server::percent_encode("docs/tasks.md"),
            "docs/tasks.md"
        );
        assert_eq!(
            weft::server::percent_encode("my plan#1.md"),
            "my%20plan%231.md"
        );
    }

    fn actor(cid: u64, role: &str) -> Actor {
        Actor {
            cid,
            name: format!("agent{cid}"),
            role: role.to_string(),
        }
    }

    fn doc(kind: &str, file: &str) -> Doc {
        Doc {
            kind: kind.to_string(),
            title: kind.to_string(),
            file: Some(file.to_string()),
            repo_path: None,
            commit: None,
            summary: format!("the {kind}"),
            by_cid: 0,
            by: String::new(),
            at_ms: 0,
        }
    }

    /// The acceptance test of Graph 3/6: a project with four linked tasks is planned, approved,
    /// posted and claimed in order.
    #[test]
    fn a_planned_project_is_posted_whole_and_its_tasks_are_claimed_in_order() {
        let dir = std::env::temp_dir().join(format!(
            "apid-task-plan-{}-{}",
            std::process::id(),
            apz_plaza::now_ms()
        ));
        let projects = Store::load(&dir);
        let ledger = TaskLedger::new(EventBus::new());
        let orchestrator = actor(1, "orchestrator");
        let planner = actor(2, "planner");
        let critic = actor(3, "reviewer");

        // Planned: the brief, the research, then the proposal and its tasks list.
        let project = projects
            .open(
                &orchestrator,
                "Release 0.9",
                "Ship release 0.9",
                "other",
                false,
            )
            .expect("opened");
        let pid = project.id.as_str();
        projects
            .attach(pid, &orchestrator, doc("brief", "f1"))
            .expect("brief");
        projects
            .advance(pid, &orchestrator, "")
            .expect("to research");
        projects
            .attach(pid, &planner, doc("research", "f2"))
            .expect("research");
        projects
            .advance(pid, &orchestrator, "")
            .expect("to proposal");
        projects
            .attach(pid, &planner, doc("proposal", "f3"))
            .expect("proposal");
        // The tasks document is checked as it is attached (projects::run does this for a file).
        let list = parse(TASKS_MD).expect("a list");
        check(&list, |_| false).expect("the list holds together");
        projects
            .attach(pid, &planner, doc("tasks", "f4"))
            .expect("tasks");
        // Posting before approval is refused, and creates nothing.
        assert!(
            post(&projects, &ledger, pid, &list, None)
                .unwrap_err()
                .contains("only once its plan is approved")
        );
        assert!(ledger.list().is_empty());
        projects.advance(pid, &planner, "").expect("to critique");
        projects
            .critique(pid, &critic, "approve", "the order is right")
            .expect("critiqued");
        projects.advance(pid, &planner, "").expect("to approval");
        // Approved.
        projects
            .approve(pid, &orchestrator, "go")
            .expect("approved");

        // Posted: all four at once, each running after what its line names.
        let posted = post(&projects, &ledger, pid, &list, None).expect("posted");
        let keyed: Vec<(&str, &str, bool)> = posted
            .iter()
            .map(|task| (task.key.as_str(), task.id.as_str(), task.ready))
            .collect();
        assert_eq!(
            keyed,
            vec![
                ("1.1", "t1", true),
                ("1.2", "t2", true),
                ("1.3", "t3", false),
                ("1.4", "t4", false),
            ]
        );
        assert_eq!(posted[3].after, vec!["t2", "t3"]);
        assert_eq!(
            projects.get(pid).expect("the project").tasks,
            vec!["t1", "t2", "t3", "t4"]
        );
        assert!(
            ledger
                .list()
                .iter()
                .all(|task| task.project() == Some(pid) && task.budget() == 2000)
        );

        // Claimed in order: one worker, each pass taking the first ready task (as the
        // scheduler's pass does) and finishing it.
        let worker = ConnectedAgent {
            cid: 100,
            role: Some("builder".to_string()),
            run: "r1".to_string(),
            vm: 0,
        };
        let mut claimed = Vec::new();
        while let Some(task) = ledger.list().into_iter().find(|task| task.is_ready()) {
            for upstream in task.after() {
                assert_eq!(
                    ledger.find(&upstream).expect("a link").state(),
                    "done",
                    "{} was claimed before {upstream} was done",
                    task.id()
                );
            }
            task.claim(&worker).expect("a ready task is claimed");
            claimed.push(task.id().to_string());
            task.finish(FinishOutcome::Done, None).expect("done");
        }
        assert_eq!(claimed, vec!["t1", "t2", "t3", "t4"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_list_that_cannot_be_posted_creates_nothing_and_names_the_task() {
        let dir = std::env::temp_dir().join(format!(
            "apid-task-plan-refused-{}-{}",
            std::process::id(),
            apz_plaza::now_ms()
        ));
        let projects = Store::load(&dir);
        let ledger = TaskLedger::new(EventBus::new());
        let owner = Actor::owner();
        let project = projects
            .open(
                &actor(1, "orchestrator"),
                "Docs",
                "Refresh the docs",
                "other",
                false,
            )
            .expect("opened");
        let pid = project.id.as_str();
        projects.approve(pid, &owner, "").unwrap_err();
        // The owner may approve from critique; get there the short way.
        for (kind, file) in [
            ("brief", "f1"),
            ("research", "f2"),
            ("proposal", "f3"),
            ("tasks", "f4"),
        ] {
            projects
                .attach(pid, &actor(2, "planner"), doc(kind, file))
                .expect("attached");
            if kind != "proposal" {
                projects.advance(pid, &owner, "").expect("advanced");
            }
        }
        projects
            .approve(pid, &owner, "")
            .expect("the owner approves");

        let mut list = vec![planned("a", "write", &[]), planned("b", "check", &["a"])];
        list[1].budget = Some(10);
        let refused = post(&projects, &ledger, pid, &list, None).unwrap_err();
        assert!(
            refused.starts_with("task b: a budget of 10 credits is below the minimum"),
            "{refused}"
        );
        list[1].budget = None;
        list[1].role = Some("r".repeat(65));
        assert_eq!(
            post(&projects, &ledger, pid, &list, None).unwrap_err(),
            "task b: role must be at most 64 bytes",
            "the ledger's refusal names the task by its key"
        );
        assert!(ledger.list().is_empty());
        assert!(projects.get(pid).expect("the project").tasks.is_empty());
        list[1].role = None;
        let posted = post(&projects, &ledger, pid, &list, None).expect("posted");
        let ids: Vec<&str> = posted.iter().map(|task| task.id.as_str()).collect();
        assert_eq!(ids, vec!["t1", "t2"], "the refused lists took no ids");
        assert_eq!(posted[1].after, vec!["t1"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
