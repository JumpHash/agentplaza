//! A project's task graph: its tasks in the order their links let them run, as
//! `GET /v1/projects/{id}/graph` and the hub MCP's `project_graph` report it.
//!
//! The ledger holds the links (each task's `after`, see `apz_plaza::tasks`); this is the read model
//! the Work screen and the TV draw from. Each task carries its depth (the longest run of links behind
//! it), its state, whether it is ready, and who is on it now: its claimant, or its reviewer while it
//! waits for a verdict, with that agent's current step ([`crate::steps`]). Beside the tasks:
//!
//! - `edges`: each link as `[from, to]`, the task that runs first and the one that runs after it;
//! - `frontier`: the tasks being worked on or ready to be (claimed, awaiting review, or ready);
//! - `longest_chain`: the longest run of linked tasks still to finish, first to last: the fewest
//!   hand-offs left before the project's tasks are done;
//! - `waiting`: the ready tasks no agent has taken yet, by the role they wait for (`null`: any worker).

use std::collections::HashMap;

use apz_plaza::tasks::TaskView;
use serde::Serialize;

use crate::state::App;
use crate::steps::Step;

/// A project's task graph.
#[derive(Debug, PartialEq, Serialize)]
pub struct Graph {
    /// The project's id.
    pub project: String,
    /// The project's stage (`brief` … `delivered`).
    pub stage: String,
    /// The project's tasks, by depth, then in the order they were posted.
    pub tasks: Vec<Node>,
    /// Each link as `[from, to]`: `to` runs after `from`.
    pub edges: Vec<[String; 2]>,
    /// The tasks being worked on or ready to be: claimed, awaiting review, or ready.
    pub frontier: Vec<String>,
    /// The longest run of linked tasks still to finish (open, claimed or awaiting review), first to
    /// last. Empty once every task has ended.
    pub longest_chain: Vec<String>,
    /// The ready tasks no agent has taken yet, by the role they wait for.
    pub waiting: Vec<Waiting>,
}

/// One task of the graph.
#[derive(Debug, PartialEq, Serialize)]
pub struct Node {
    pub id: String,
    pub goal: String,
    pub role: Option<String>,
    /// 0 for a task that runs after none of the project's tasks, else one more than the deepest task
    /// it runs after.
    pub depth: usize,
    pub state: &'static str,
    pub ready: bool,
    /// While the task is open: the tasks it runs after that failed or were withdrawn.
    pub blocked_by: Vec<String>,
    /// The agent that claimed it.
    pub agent_cid: Option<u64>,
    /// The reviewer on it, while it awaits a verdict.
    pub reviewer_cid: Option<u64>,
    /// What the agent on it is doing now: its claimant's step while it is claimed, its reviewer's
    /// while it awaits a verdict.
    pub step: Option<Step>,
}

/// Ready tasks no agent has taken yet, for one role.
#[derive(Debug, PartialEq, Serialize)]
pub struct Waiting {
    /// The role they wait for, or `null` for tasks any worker may take.
    pub role: Option<String>,
    pub tasks: Vec<String>,
}

/// The graph of project `id`, or `None` when there is no such project.
pub fn of_project(app: &App, id: &str) -> Option<Graph> {
    let project = app.projects.get(id)?;
    let tasks = crate::plaza::project_tasks(app, &project.id);
    let views: Vec<TaskView<'_>> = tasks.iter().map(|task| task.view()).collect();
    Some(build(
        &project.id,
        &project.stage,
        &views,
        |task| app.work.reviewer_of(task),
        crate::steps::of,
    ))
}

/// The graph of `tasks`, one project's, oldest first. `reviewer_of` names the reviewer on a task
/// that awaits a verdict, and `step_of` an agent's current step.
pub fn build(
    project: &str,
    stage: &str,
    tasks: &[TaskView<'_>],
    reviewer_of: impl Fn(&str) -> Option<u64>,
    step_of: impl Fn(u64) -> Option<Step>,
) -> Graph {
    let index: HashMap<&str, usize> = tasks
        .iter()
        .enumerate()
        .map(|(at, task)| (task.id, at))
        .collect();
    // Each task's links, as indexes into `tasks`. A link stays inside its project, so a link to a
    // task outside these is one no ledger makes; it is left out.
    let links: Vec<Vec<usize>> = tasks
        .iter()
        .map(|task| {
            task.after
                .iter()
                .filter_map(|id| index.get(id.as_str()).copied())
                .collect()
        })
        .collect();
    let depth = depths(&links);
    let mut order: Vec<usize> = (0..tasks.len()).collect();
    order.sort_by_key(|&at| (depth[at], at));

    let unfinished = |at: usize| matches!(tasks[at].state, "open" | "claimed" | "awaiting_review");
    let mut nodes = Vec::with_capacity(tasks.len());
    let mut edges = Vec::new();
    let mut frontier = Vec::new();
    let mut waiting: Vec<Waiting> = Vec::new();
    for &at in &order {
        let task = &tasks[at];
        for &from in &links[at] {
            edges.push([tasks[from].id.to_string(), task.id.to_string()]);
        }
        let reviewer_cid = (task.state == "awaiting_review")
            .then(|| reviewer_of(task.id))
            .flatten();
        let step = match task.state {
            "claimed" => task.agent_cid.and_then(&step_of),
            "awaiting_review" => reviewer_cid.and_then(&step_of),
            _ => None,
        };
        if task.ready || matches!(task.state, "claimed" | "awaiting_review") {
            frontier.push(task.id.to_string());
        }
        if task.ready {
            let role = task.role.map(str::to_string);
            match waiting.iter_mut().find(|group| group.role == role) {
                Some(group) => group.tasks.push(task.id.to_string()),
                None => waiting.push(Waiting {
                    role,
                    tasks: vec![task.id.to_string()],
                }),
            }
        }
        nodes.push(Node {
            id: task.id.to_string(),
            goal: task.goal.to_string(),
            role: task.role.map(str::to_string),
            depth: depth[at],
            state: task.state,
            ready: task.ready,
            blocked_by: task.blocked_by.clone(),
            agent_cid: task.agent_cid,
            reviewer_cid,
            step,
        });
    }

    // The longest chain of unfinished tasks: in depth order every link comes before the task that
    // runs after it, so one pass finds, for each task, the longest chain that ends with it.
    let mut longest: Vec<usize> = vec![0; tasks.len()];
    let mut previous: Vec<Option<usize>> = vec![None; tasks.len()];
    for &at in &order {
        if !unfinished(at) {
            continue;
        }
        longest[at] = 1;
        for &from in &links[at] {
            if unfinished(from) && longest[from] + 1 > longest[at] {
                longest[at] = longest[from] + 1;
                previous[at] = Some(from);
            }
        }
    }
    let mut longest_chain = Vec::new();
    let end = order.iter().copied().filter(|&at| longest[at] > 0).fold(
        None,
        |best: Option<usize>, at| match best {
            Some(best) if longest[best] >= longest[at] => Some(best),
            _ => Some(at),
        },
    );
    let mut at = end;
    while let Some(step) = at {
        longest_chain.push(tasks[step].id.to_string());
        at = previous[step];
    }
    longest_chain.reverse();

    Graph {
        project: project.to_string(),
        stage: stage.to_string(),
        tasks: nodes,
        edges,
        frontier,
        longest_chain,
        waiting,
    }
}

/// Each task's depth: 0 with no links, else one more than its deepest link's. The ledger keeps the
/// links free of cycles; a cycle met anyway is cut where the walk finds it, so this always ends.
fn depths(links: &[Vec<usize>]) -> Vec<usize> {
    let mut depth: Vec<Option<usize>> = vec![None; links.len()];
    let mut walking = vec![false; links.len()];
    for start in 0..links.len() {
        let mut stack = vec![start];
        while let Some(&task) = stack.last() {
            if depth[task].is_some() {
                stack.pop();
                continue;
            }
            walking[task] = true;
            let pending: Vec<usize> = links[task]
                .iter()
                .copied()
                .filter(|&from| depth[from].is_none() && !walking[from])
                .collect();
            if pending.is_empty() {
                depth[task] = Some(
                    links[task]
                        .iter()
                        .filter_map(|&from| depth[from])
                        .map(|from| from + 1)
                        .max()
                        .unwrap_or(0),
                );
                walking[task] = false;
                stack.pop();
            } else {
                stack.extend(pending);
            }
        }
    }
    depth.into_iter().map(Option::unwrap_or_default).collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use apz_plaza::ConnectedAgent;
    use apz_plaza::events::EventBus;
    use apz_plaza::tasks::{CreateTask, FinishOutcome, Task, TaskLedger};

    use super::*;

    fn agent(cid: u64, role: &str) -> ConnectedAgent {
        ConnectedAgent {
            cid,
            role: Some(role.to_string()),
            run: "r1".to_string(),
            vm: 0,
        }
    }

    fn task(ledger: &TaskLedger, goal: &str, role: Option<&str>, after: &[&str]) -> Arc<Task> {
        ledger
            .create(&CreateTask {
                goal: goal.to_string(),
                acceptance: None,
                budget: None,
                role: role.map(str::to_string),
                after: after.iter().map(|id| id.to_string()).collect(),
                project: Some("pr1".to_string()),
            })
            .expect("a task")
    }

    fn step(stage: &'static str, tool: Option<&str>) -> Step {
        Step {
            stage,
            tool: tool.map(str::to_string),
            since_ms: 1,
        }
    }

    #[test]
    fn a_release_plan_is_laid_out_by_its_links() {
        // A release: the scheduler fix and the sanction fix side by side; release notes after the
        // first, a security review after the second, QA after the sanction fix and its review, and
        // the release after the notes and QA. Plus docs that run after nothing.
        let ledger = TaskLedger::new(EventBus::new());
        let scheduler = task(&ledger, "Patch the scheduler test", Some("builder"), &[]);
        let sanction = task(&ledger, "Land the sanction fix", Some("builder"), &[]);
        task(&ledger, "Write the release notes", Some("release"), &["t1"]);
        task(
            &ledger,
            "Review the sanction fix",
            Some("security"),
            &["t2"],
        );
        task(&ledger, "QA on staging", Some("qa"), &["t2", "t4"]);
        task(
            &ledger,
            "Publish the release",
            Some("release"),
            &["t3", "t5"],
        );
        task(&ledger, "Refresh the docs", None, &[]);
        // The scheduler fix is accepted; the sanction fix waits for its review; the notes are being
        // written.
        scheduler.claim(&agent(107, "builder")).expect("claim");
        scheduler.finish(FinishOutcome::Done, None).expect("done");
        sanction.claim(&agent(106, "builder")).expect("claim");
        sanction.await_review(None).expect("awaiting review");
        ledger
            .find("t3")
            .expect("t3")
            .claim(&agent(116, "release"))
            .expect("claim");

        let tasks = ledger.list();
        let views: Vec<TaskView<'_>> = tasks.iter().map(|task| task.view()).collect();
        let graph = build(
            "pr1",
            "implementation",
            &views,
            |task| (task == "t2").then_some(111),
            |cid| match cid {
                116 => Some(step("tool", Some("bash"))),
                111 => Some(step("thinking", None)),
                _ => None,
            },
        );

        assert_eq!(graph.project, "pr1");
        assert_eq!(graph.stage, "implementation");
        let laid_out: Vec<(&str, usize)> = graph
            .tasks
            .iter()
            .map(|node| (node.id.as_str(), node.depth))
            .collect();
        assert_eq!(
            laid_out,
            vec![
                ("t1", 0),
                ("t2", 0),
                ("t7", 0),
                ("t3", 1),
                ("t4", 1),
                ("t5", 2),
                ("t6", 3),
            ]
        );
        let edge = |from: &str, to: &str| [from.to_string(), to.to_string()];
        assert_eq!(
            graph.edges,
            vec![
                edge("t1", "t3"),
                edge("t2", "t4"),
                edge("t2", "t5"),
                edge("t4", "t5"),
                edge("t3", "t6"),
                edge("t5", "t6"),
            ]
        );
        assert_eq!(graph.frontier, vec!["t2", "t7", "t3"]);
        // t2 → t4 → t5 → t6 is the longest run still to finish; t1 is done.
        assert_eq!(graph.longest_chain, vec!["t2", "t4", "t5", "t6"]);
        assert_eq!(
            graph.waiting,
            vec![Waiting {
                role: None,
                tasks: vec!["t7".to_string()],
            }],
            "the review waits on the sanction fix, so only the docs wait for an agent"
        );

        let node = |id: &str| graph.tasks.iter().find(|node| node.id == id).expect(id);
        assert_eq!(node("t2").state, "awaiting_review");
        assert_eq!(node("t2").agent_cid, Some(106));
        assert_eq!(node("t2").reviewer_cid, Some(111));
        assert_eq!(
            node("t2").step,
            Some(step("thinking", None)),
            "a task in review shows its reviewer's step"
        );
        assert_eq!(node("t3").step, Some(step("tool", Some("bash"))));
        assert_eq!(node("t3").reviewer_cid, None);
        assert_eq!(node("t1").step, None, "a done task has nobody on it");
        assert!(!node("t4").ready);
        assert_eq!(node("t4").role.as_deref(), Some("security"));
        assert_eq!(node("t7").goal, "Refresh the docs");
    }

    #[test]
    fn blocked_and_finished_work_shapes_the_frontier_and_the_chain() {
        let ledger = TaskLedger::new(EventBus::new());
        let flaky = task(&ledger, "flaky", Some("builder"), &[]);
        task(&ledger, "after the flaky one", Some("qa"), &["t1"]);
        task(&ledger, "ready for QA", Some("qa"), &[]);
        task(&ledger, "also for QA", Some("qa"), &[]);
        flaky.claim(&agent(100, "builder")).expect("claim");
        flaky.finish(FinishOutcome::Failed, None).expect("failed");

        let tasks = ledger.list();
        let views: Vec<TaskView<'_>> = tasks.iter().map(|task| task.view()).collect();
        let graph = build("pr1", "implementation", &views, |_| None, |_| None);
        let blocked = graph.tasks.iter().find(|node| node.id == "t2").expect("t2");
        assert_eq!(blocked.blocked_by, vec!["t1"]);
        assert!(!blocked.ready);
        assert_eq!(graph.frontier, vec!["t3", "t4"]);
        assert_eq!(
            graph.waiting,
            vec![Waiting {
                role: Some("qa".to_string()),
                tasks: vec!["t3".to_string(), "t4".to_string()],
            }]
        );
        // The failed task has ended; the blocked one is still to finish, a chain of one.
        assert_eq!(graph.longest_chain.len(), 1);
        assert_eq!(graph.edges, vec![["t1".to_string(), "t2".to_string()]]);
    }

    #[test]
    fn a_graph_with_no_tasks_or_none_left_is_empty_where_it_should_be() {
        let graph = build("pr1", "research", &[], |_| None, |_| None);
        assert!(graph.tasks.is_empty() && graph.edges.is_empty());
        assert!(graph.frontier.is_empty() && graph.longest_chain.is_empty());
        assert!(graph.waiting.is_empty());

        let ledger = TaskLedger::new(EventBus::new());
        let only = task(&ledger, "only", None, &[]);
        only.claim(&agent(100, "builder")).expect("claim");
        only.finish(FinishOutcome::Done, None).expect("done");
        let tasks = ledger.list();
        let views: Vec<TaskView<'_>> = tasks.iter().map(|task| task.view()).collect();
        let graph = build("pr1", "review", &views, |_| None, |_| None);
        assert!(graph.longest_chain.is_empty(), "nothing is left to finish");
        assert!(graph.frontier.is_empty());
    }

    #[test]
    fn a_relinked_task_sits_after_a_later_one() {
        // Links are not in creation order once a task is relinked: t1 now runs after t2.
        let ledger = TaskLedger::new(EventBus::new());
        task(&ledger, "first posted", None, &[]);
        task(&ledger, "second posted", None, &[]);
        ledger.relink("t1", &["t2".to_string()]).expect("relinked");
        let tasks = ledger.list();
        let views: Vec<TaskView<'_>> = tasks.iter().map(|task| task.view()).collect();
        let graph = build("pr1", "implementation", &views, |_| None, |_| None);
        let laid_out: Vec<(&str, usize)> = graph
            .tasks
            .iter()
            .map(|node| (node.id.as_str(), node.depth))
            .collect();
        assert_eq!(laid_out, vec![("t2", 0), ("t1", 1)]);
        assert_eq!(graph.longest_chain, vec!["t2", "t1"]);
    }

    #[test]
    fn depths_end_even_on_a_cycle() {
        // The ledger never makes one; the walk still ends if it meets one.
        let cycle = vec![vec![1], vec![0], vec![1]];
        let depth = depths(&cycle);
        assert_eq!(depth.len(), 3);
        assert!(depth[2] > depth[1]);
    }
}
