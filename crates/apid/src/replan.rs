//! When a task fails or is withdrawn, the open tasks that run after it are blocked. They stay open
//! (nothing fails on its own) until someone replans them, and the agent that can is told: the
//! project's orchestrator (its lead) for a project's tasks, else the agent that posted them. It gets
//! a chat item (the project's chat, when its draft started there), as a poster hears that its task ended. The
//! item names the task that ended, the tasks it holds up, and the three ways on: relink, repost, or
//! withdraw.

use std::sync::Arc;

use apz_plaza::events::Event;
use apz_plaza::tasks::Task;
use apz_proto::wire::WorkKind;

use crate::scheduler::Origin;
use crate::state::App;

/// The longest goal a notice quotes, in characters.
const GOAL_CHARS: usize = 80;

/// Starts the thread that tells the replanner of each task that fails or is withdrawn while
/// open tasks run after it.
pub fn spawn(app: Arc<App>) {
    let events = app.plaza.events.subscribe();
    let spawned = std::thread::Builder::new()
        .name("apid-replan".into())
        .spawn(move || {
            for event in events {
                if let Event::Task { id, state, .. } = event
                    && matches!(state, "failed" | "withdrawn")
                {
                    tell(&app, &id, state);
                }
            }
        });
    if let Err(err) = spawned {
        eprintln!("replan: cannot start: {err}");
    }
}

/// Queues the notices for task `ended`, which ended as `state`.
fn tell(app: &App, ended: &str, state: &str) {
    let tasks = app.plaza.tasks.list();
    let Some(task) = tasks.iter().find(|task| task.id() == ended) else {
        return;
    };
    for (cid, prompt) in notices(task, state, &tasks, |blocked| replanner(app, blocked)) {
        // A replanner that does not work now (its project is paused or over) is not told; the owner
        // sees the task blocked on its view.
        if crate::projects::held_cid(app, cid).is_some() {
            continue;
        }
        // A project whose draft started hears of it in its chat.
        if let Some(chat) = crate::scheduler::project_chat(app, ended, cid) {
            crate::work_loop::tell_chat(
                app,
                &chat,
                cid,
                crate::work_loop::shown_line(&prompt),
                &prompt,
            );
            continue;
        }
        app.work.enqueue(
            app,
            cid,
            WorkKind::Chat,
            format!("op-{cid}"),
            prompt,
            Origin::default(),
        );
    }
}

/// Who replans a blocked task: its project's lead, else the agent that posted it.
fn replanner(app: &App, task: &Task) -> Option<u64> {
    task.project()
        .and_then(|pid| app.projects.get(pid))
        .map(|project| project.lead_cid)
        .filter(|&cid| cid != 0)
        .or_else(|| app.work.poster_of(task.id()))
}

/// The notices for `ended`, which ended as `state`: one per replanner, naming the open tasks that
/// run after `ended` and that it replans. A blocked task nobody can replan (the owner's own) is in
/// none; the owner sees it blocked on the task's view.
pub(crate) fn notices(
    ended: &Task,
    state: &str,
    tasks: &[Arc<Task>],
    replanner: impl Fn(&Task) -> Option<u64>,
) -> Vec<(u64, String)> {
    let mut groups: Vec<(u64, Vec<&Task>)> = Vec::new();
    for task in tasks {
        if !task.is_open() || !task.after().iter().any(|id| id == ended.id()) {
            continue;
        }
        let Some(cid) = replanner(task) else {
            continue;
        };
        match groups.iter_mut().find(|(who, _)| *who == cid) {
            Some((_, blocked)) => blocked.push(task),
            None => groups.push((cid, vec![task])),
        }
    }
    let how = if state == "withdrawn" {
        "was withdrawn"
    } else {
        "failed"
    };
    groups
        .into_iter()
        .map(|(cid, blocked)| {
            let held: Vec<String> = blocked
                .iter()
                .map(|task| format!("{} ({})", task.id(), quoted(task.goal())))
                .collect();
            let prompt = format!(
                "[plaza] Task {} ({}) {how}. These open tasks run after it and are blocked until they are replanned: {}.\n\n\
                 Nothing fails on its own. For each, relink it with plaza_task_link (the whole new list of what it runs \
                 after), post a replacement with plaza_post_task and link it to that, or withdraw it with plaza_cancel_task.",
                ended.id(),
                quoted(ended.goal()),
                held.join(", ")
            );
            (cid, prompt)
        })
        .collect()
}

/// A goal's first line in quotes, cut to [`GOAL_CHARS`].
fn quoted(goal: &str) -> String {
    let line = goal.lines().next().unwrap_or_default().trim();
    let mut cut: String = line.chars().take(GOAL_CHARS).collect();
    if line.chars().count() > GOAL_CHARS {
        cut.push('…');
    }
    format!("\"{cut}\"")
}

#[cfg(test)]
mod tests {
    use apz_plaza::ConnectedAgent;
    use apz_plaza::events::EventBus;
    use apz_plaza::tasks::{CreateTask, FinishOutcome, TaskLedger};

    use super::*;

    fn task(ledger: &TaskLedger, goal: &str, after: &[&str], project: Option<&str>) -> Arc<Task> {
        ledger
            .create(&CreateTask {
                goal: goal.to_string(),
                acceptance: None,
                budget: None,
                role: None,
                after: after.iter().map(|id| id.to_string()).collect(),
                project: project.map(str::to_string),
            })
            .expect("a task")
    }

    #[test]
    fn a_failed_task_tells_each_replanner_what_it_holds_up() {
        let ledger = TaskLedger::new(EventBus::new());
        let fix = task(
            &ledger,
            "Land the sanction fix\nwith tests",
            &[],
            Some("pr1"),
        );
        task(&ledger, "QA on staging", &["t1"], Some("pr1"));
        task(&ledger, "Publish the release", &["t1"], Some("pr1"));
        let loose = task(&ledger, "Loose upstream", &[], None);
        task(&ledger, "After the loose one", &["t4"], None);
        let dropped = task(&ledger, "Already withdrawn", &["t1"], Some("pr1"));
        dropped.withdraw().expect("withdrawn");
        fix.claim(&ConnectedAgent {
            cid: 100,
            role: Some("builder".to_string()),
            run: "r1".to_string(),
            vm: 0,
        })
        .expect("claim");
        fix.finish(FinishOutcome::Failed, None).expect("failed");

        let lead = |task: &Task| (task.project() == Some("pr1")).then_some(7);
        let told = notices(&fix, "failed", &ledger.list(), lead);
        assert_eq!(told.len(), 1, "one notice for the project's lead");
        let (cid, prompt) = &told[0];
        assert_eq!(*cid, 7);
        assert!(
            prompt.starts_with("[plaza] Task t1 (\"Land the sanction fix\") failed. These open tasks run after it and are blocked until they are replanned: t2 (\"QA on staging\"), t3 (\"Publish the release\")."),
            "{prompt}"
        );
        assert!(prompt.contains("plaza_task_link") && prompt.contains("plaza_cancel_task"));
        assert!(!prompt.contains("t6"), "a withdrawn task is not blocked");

        // A task outside a project goes to its poster; with none, nobody is told.
        loose.withdraw().expect("withdrawn");
        let poster = |task: &Task| (task.id() == "t5").then_some(9);
        let told = notices(&loose, "withdrawn", &ledger.list(), poster);
        assert_eq!(told.len(), 1);
        assert_eq!(told[0].0, 9);
        assert!(told[0].1.contains("(\"Loose upstream\") was withdrawn"));
        assert!(notices(&loose, "withdrawn", &ledger.list(), |_| None).is_empty());
    }

    #[test]
    fn a_long_goal_is_cut_in_the_notice() {
        let long = "x".repeat(GOAL_CHARS + 10);
        let shown = quoted(&long);
        assert_eq!(shown.chars().count(), GOAL_CHARS + 3);
        assert!(shown.ends_with("…\""));
    }
}
