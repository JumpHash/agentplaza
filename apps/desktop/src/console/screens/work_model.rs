//! The Work screen's pure parts, as the web's `apps/web/src/views/work-model.ts` has them: what a task is to the
//! screen, the line each card says, a project's flow line, the plan's cards, the tasks "Only what needs me" keeps,
//! and where the plan, the graph and delivery stand on the map. The screen (`work.rs`) draws from these, and the
//! tests below check them against the web's cases, so both consoles say the same things.

use std::collections::{HashMap, HashSet};

use serde::Deserialize;

use crate::console::roles;

/// The eight stages of a project, in order (`PROJECT_STAGES` on the web).
pub const STAGES: [&str; 8] = [
    "brief",
    "research",
    "proposal",
    "critique",
    "approval",
    "implementation",
    "review",
    "delivered",
];

/// The planning stages, in order: their cards make the Plan column.
pub const PLAN_STAGES: [&str; 5] = ["brief", "research", "proposal", "critique", "approval"];

/// The most task squares a flow line draws; the rest are counted.
pub const FLOW_TASKS: usize = 24;

/// A stage's place among the eight; an unknown stage counts as the first.
pub fn stage_index(stage: &str) -> usize {
    STAGES.iter().position(|s| *s == stage).unwrap_or(0)
}

// ---- The hub's shapes (as apps/web/src/api.ts types them) ----------------------------------------------------------

/// A project (`GET /v1/projects`), with what the Work screen reads of it.
#[derive(Debug, Clone, Deserialize)]
pub struct Project {
    pub id: String,
    pub title: String,
    pub stage: String,
    /// The plan waits for the owner's approval, not the orchestrator's.
    #[serde(default)]
    pub owner_approval: bool,
    #[serde(default)]
    pub approved_by: Option<String>,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub docs: Vec<Doc>,
    #[serde(default)]
    pub critiques: Vec<Critique>,
    #[serde(default)]
    pub tasks: Vec<String>,
    #[serde(default)]
    pub updated_ms: i64,
    /// Paused: its agents take no work until it is resumed.
    #[serde(default)]
    pub paused: bool,
    /// Archived: nothing more is done for it.
    #[serde(default)]
    pub archived: bool,
}

impl Project {
    /// Whether it still has work: not archived, not delivered.
    pub fn live(&self) -> bool {
        !self.archived && self.stage != "delivered"
    }
}

/// A document a planning stage produced.
#[derive(Debug, Clone, Deserialize)]
pub struct Doc {
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub by_cid: u64,
    #[serde(default)]
    pub at_ms: i64,
}

/// A critique of the plan.
#[derive(Debug, Clone, Deserialize)]
pub struct Critique {
    pub verdict: String,
    #[serde(default)]
    pub by_cid: u64,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub at_ms: i64,
}

/// A task as the ledger lists it (`GET /v1/tasks`).
#[derive(Debug, Clone, Deserialize)]
pub struct Task {
    pub id: String,
    pub state: String,
    pub goal: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    /// The tasks it runs after.
    #[serde(default)]
    pub after: Vec<String>,
    /// Open, and every task it runs after is done.
    #[serde(default)]
    pub ready: bool,
    /// The tasks it runs after that failed or were withdrawn.
    #[serde(default)]
    pub blocked_by: Vec<String>,
    #[serde(default)]
    pub agent_cid: Option<u64>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

/// What an agent is doing now: the `agent_step` event's `step`, and an agent's `step` in the roster.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Step {
    /// thinking, streaming, tool, approval, idle or asleep.
    pub stage: String,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub since_ms: i64,
}

/// A task of a project's graph (`GET /v1/projects/{id}/graph`).
#[derive(Debug, Clone, Deserialize)]
pub struct GraphTask {
    pub id: String,
    pub goal: String,
    #[serde(default)]
    pub role: Option<String>,
    pub state: String,
    #[serde(default)]
    pub ready: bool,
    #[serde(default)]
    pub blocked_by: Vec<String>,
    #[serde(default)]
    pub agent_cid: Option<u64>,
    #[serde(default)]
    pub reviewer_cid: Option<u64>,
}

/// A project's graph: its tasks and their links as `[from, to]`, `to` running after `from`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Graph {
    #[serde(default)]
    pub tasks: Vec<GraphTask>,
    #[serde(default)]
    pub edges: Vec<[String; 2]>,
}

// ---- What a task is to the screen -----------------------------------------------------------------------------------

/// The hub's states, an open task split by its links, and a claim split by its agent's step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkState {
    Ready,
    Waiting,
    Blocked,
    Working,
    NeedsYou,
    Review,
    Done,
    Failed,
    Withdrawn,
}

impl WorkState {
    /// The task's state on the screen. An open task is ready, waiting on its links, or blocked by one that failed; a
    /// claim whose agent waits on the owner's decision needs you.
    pub fn of(state: &str, ready: bool, blocked_by: &[String], step: Option<&Step>) -> WorkState {
        match state {
            "open" if !blocked_by.is_empty() => WorkState::Blocked,
            "open" if ready => WorkState::Ready,
            "open" => WorkState::Waiting,
            "claimed" if step.is_some_and(|s| s.stage == "approval") => WorkState::NeedsYou,
            "awaiting_review" => WorkState::Review,
            "done" => WorkState::Done,
            "failed" => WorkState::Failed,
            "withdrawn" => WorkState::Withdrawn,
            _ => WorkState::Working,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            WorkState::Ready => "Ready",
            WorkState::Waiting => "Waiting",
            WorkState::Blocked => "Blocked",
            WorkState::Working => "Working",
            WorkState::NeedsYou => "Needs you",
            WorkState::Review => "In review",
            WorkState::Done => "Done",
            WorkState::Failed => "Failed",
            WorkState::Withdrawn => "Withdrawn",
        }
    }

    /// Whether the state is over: its card dims in place.
    pub fn ended(self) -> bool {
        matches!(
            self,
            WorkState::Done | WorkState::Failed | WorkState::Withdrawn
        )
    }
}

/// A goal's first line: the card's title.
pub fn title_of(goal: &str) -> String {
    goal.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("Untitled task")
        .to_string()
}

/// Items as a sentence lists them: `a`, `a and b`, `a, b and c`.
pub fn listed(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// A span said short: `40s`, `12m`, `3h`, `2d`.
pub fn short_span(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m", s / 60),
        3600..=86_399 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86_400),
    }
}

/// What an agent's step says on a card: `Running bash · 2m`. Empty without a step.
pub fn step_text(step: Option<&Step>, now: i64) -> String {
    let Some(step) = step else {
        return String::new();
    };
    let what = match (step.stage.as_str(), step.tool.as_deref()) {
        ("thinking", _) => "Thinking".to_string(),
        ("streaming", _) => "Writing".to_string(),
        ("tool", Some(tool)) => format!("Running {tool}"),
        ("tool", None) => "Running tools".to_string(),
        ("approval", Some(tool)) => format!("Waits for you to approve {tool}"),
        ("approval", None) => "Waits for your approval".to_string(),
        ("idle", _) => "Idle".to_string(),
        ("asleep", _) => "Asleep".to_string(),
        (other, _) => other.to_string(),
    };
    format!("{what} · {}", short_span(now - step.since_ms))
}

/// The tasks `id` runs after, from the links.
pub fn upstream_of(id: &str, edges: &[[String; 2]]) -> Vec<String> {
    edges
        .iter()
        .filter(|[_, to]| to == id)
        .map(|[from, _]| from.clone())
        .collect()
}

/// Who can claim a task: `QA agent`, or `agent` when it names no role. Role labels are names (Incident, Release), so
/// the wording never puts an article or a plural on them.
pub fn claimant(role: Option<&str>) -> String {
    match role {
        Some(role) => format!("{} agent", roles::label(role)),
        None => "agent".to_string(),
    }
}

/// What a card needs to say who has the task or what it waits for.
pub struct LineContext<'a> {
    pub edges: &'a [[String; 2]],
    /// A task's hub state by id, for the links it waits on.
    pub state_of: &'a dyn Fn(&str) -> Option<String>,
    /// How the line names another task: a card its id (that card is on the same map), the drawer its title.
    pub task_name: &'a dyn Fn(&str) -> String,
    /// When a ready task became ready (see `ready_since`), when the screen knows.
    pub ready_at: &'a dyn Fn(&str) -> Option<i64>,
    pub label_of: &'a dyn Fn(u64) -> String,
    pub step_of: &'a dyn Fn(u64) -> Option<Step>,
    pub now: i64,
}

/// The line under a card's title: who has the task and what they do now, or what it waits for.
pub fn card_line(t: &GraphTask, state: WorkState, ctx: &LineContext<'_>) -> String {
    match state {
        WorkState::Ready => match (ctx.ready_at)(&t.id) {
            Some(at) => format!("Unclaimed for {}", short_span(ctx.now - at)),
            None => format!("Any {} can claim it", claimant(t.role.as_deref())),
        },
        WorkState::Waiting => {
            let open: Vec<String> = upstream_of(&t.id, ctx.edges)
                .into_iter()
                .filter(|id| (ctx.state_of)(id).as_deref() != Some("done"))
                .map(|id| (ctx.task_name)(&id))
                .collect();
            if open.is_empty() {
                "Waiting".to_string()
            } else {
                format!("After {}", listed(&open))
            }
        }
        WorkState::Blocked => {
            let names: Vec<String> = t.blocked_by.iter().map(|id| (ctx.task_name)(id)).collect();
            format!("Blocked: {} did not finish", listed(&names))
        }
        WorkState::Working => match t.agent_cid {
            None => state.label().to_string(),
            Some(cid) => {
                let step = step_text((ctx.step_of)(cid).as_ref(), ctx.now);
                if step.is_empty() {
                    (ctx.label_of)(cid)
                } else {
                    format!("{} · {step}", (ctx.label_of)(cid))
                }
            }
        },
        // The decision first: the state already says it waits on the owner, and a long line is cut at its end.
        WorkState::NeedsYou => match t.agent_cid {
            None => state.label().to_string(),
            Some(cid) => {
                let step = (ctx.step_of)(cid);
                let what = match step.as_ref().and_then(|s| s.tool.as_deref()) {
                    Some(tool) => format!("Approve {tool}"),
                    None => "Your approval".to_string(),
                };
                match step {
                    Some(s) => format!(
                        "{what} · {} · {}",
                        (ctx.label_of)(cid),
                        short_span(ctx.now - s.since_ms)
                    ),
                    None => format!("{what} · {}", (ctx.label_of)(cid)),
                }
            }
        },
        WorkState::Review => match t.reviewer_cid {
            None => "Waits for a reviewer".to_string(),
            Some(cid) => {
                let step = step_text((ctx.step_of)(cid).as_ref(), ctx.now);
                if step.is_empty() {
                    format!("Reviewed by {}", (ctx.label_of)(cid))
                } else {
                    format!("Reviewed by {} · {step}", (ctx.label_of)(cid))
                }
            }
        },
        WorkState::Done | WorkState::Failed | WorkState::Withdrawn => match t.agent_cid {
            None => state.label().to_string(),
            Some(cid) => format!("{} · {}", state.label(), (ctx.label_of)(cid)),
        },
    }
}

// ---- Flow lines ------------------------------------------------------------------------------------------------------

/// What a square of a flow line stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegKind {
    /// A planning stage.
    Plan,
    Task,
    /// Review, then delivery.
    End,
}

/// A square's state: a stage's place, the owner's, or a task's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegState {
    Done,
    Current,
    Later,
    /// The stage waits on the owner.
    You,
    Task(WorkState),
}

/// One square of a flow line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seg {
    pub kind: SegKind,
    pub state: SegState,
    pub label: String,
}

/// The tasks a project holds: the ones that name it, and the ones its record lists.
pub fn tasks_of<'a>(p: &Project, tasks: &'a [Task]) -> Vec<&'a Task> {
    tasks
        .iter()
        .filter(|t| t.project.as_deref() == Some(p.id.as_str()) || p.tasks.contains(&t.id))
        .collect()
}

/// A task's state, with its agent's step when the screen knows it.
fn task_state(t: &Task, step_of: &dyn Fn(u64) -> Option<Step>) -> WorkState {
    let step = t.agent_cid.and_then(step_of);
    WorkState::of(&t.state, t.ready, &t.blocked_by, step.as_ref())
}

/// A project's flow line: its five planning stages, a square per task, then review and delivery. For tasks outside
/// projects (`project` None), the task squares alone. With the agents' steps, a task whose agent waits on the owner is
/// the owner's square.
pub fn flow_segments(
    project: Option<&Project>,
    tasks: &[&Task],
    step_of: &dyn Fn(u64) -> Option<Step>,
) -> Vec<Seg> {
    let mut segs = Vec::new();
    let at = project.map(|p| stage_index(&p.stage));
    let place = |i: usize| match at {
        Some(at) if i < at => SegState::Done,
        Some(at) if i == at => SegState::Current,
        _ => SegState::Later,
    };
    if let Some(p) = project {
        for s in PLAN_STAGES {
            let yours = s == "approval" && plan_waits_for_owner(p);
            segs.push(Seg {
                kind: SegKind::Plan,
                state: if yours {
                    SegState::You
                } else {
                    place(stage_index(s))
                },
                label: if yours {
                    "approval · waits for you".to_string()
                } else {
                    s.to_string()
                },
            });
        }
    }
    for t in tasks.iter().take(FLOW_TASKS) {
        let state = task_state(t, step_of);
        segs.push(Seg {
            kind: SegKind::Task,
            state: SegState::Task(state),
            label: format!("{} · {}", t.id, state.label()),
        });
    }
    if let Some(p) = project {
        segs.push(Seg {
            kind: SegKind::End,
            state: place(stage_index("review")),
            label: "review".to_string(),
        });
        segs.push(Seg {
            kind: SegKind::End,
            state: if p.stage == "delivered" {
                SegState::Done
            } else {
                SegState::Later
            },
            label: "delivered".to_string(),
        });
    }
    segs
}

/// Whether a project's plan waits on the owner: it asked for the owner's approval and is at critique or approval.
pub fn plan_waits_for_owner(p: &Project) -> bool {
    p.approved_by.is_none()
        && p.owner_approval
        && matches!(p.stage.as_str(), "critique" | "approval")
}

/// Whether a project (or, with None, the tasks outside projects) waits on the owner: the plan's approval, or a task
/// whose agent waits on the owner's decision.
pub fn waits_for_owner(
    p: Option<&Project>,
    tasks: &[&Task],
    step_of: &dyn Fn(u64) -> Option<Step>,
) -> bool {
    p.is_some_and(plan_waits_for_owner)
        || tasks
            .iter()
            .any(|t| task_state(t, step_of) == WorkState::NeedsYou)
}

/// The ready tasks nobody has claimed, for one role (None: any agent): how many, and the longest wait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleWait {
    pub role: Option<String>,
    pub count: usize,
    pub waited: i64,
}

/// When a ready task became ready: when it was posted, or when the last task it runs after finished.
pub fn ready_since(t: &Task, by_id: &HashMap<&str, &Task>) -> i64 {
    t.after
        .iter()
        .map(|id| by_id.get(id.as_str()).map_or(0, |up| up.updated_at))
        .fold(t.created_at, i64::max)
}

/// The ready tasks nobody has claimed, by the role they wait for, longest wait first: where the plaza is short of an
/// agent.
pub fn waits_by_role(tasks: &[Task], now: i64) -> Vec<RoleWait> {
    let by_id: HashMap<&str, &Task> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();
    let mut groups: Vec<RoleWait> = Vec::new();
    for t in tasks.iter().filter(|t| t.state == "open" && t.ready) {
        let waited = now - ready_since(t, &by_id);
        match groups.iter_mut().find(|g| g.role == t.role) {
            Some(g) => {
                g.count += 1;
                g.waited = g.waited.max(waited);
            }
            None => groups.push(RoleWait {
                role: t.role.clone(),
                count: 1,
                waited,
            }),
        }
    }
    groups.sort_by(|a, b| b.waited.cmp(&a.waited).then(b.count.cmp(&a.count)));
    groups
}

/// The tasks "Only what needs me" keeps: the ones that wait on the owner, and every task that runs after them.
pub fn needs_me(states: &HashMap<String, WorkState>, edges: &[[String; 2]]) -> HashSet<String> {
    let mut kept = HashSet::new();
    let mut next: Vec<String> = states
        .iter()
        .filter(|(_, s)| **s == WorkState::NeedsYou)
        .map(|(id, _)| id.clone())
        .collect();
    while let Some(id) = next.pop() {
        if !kept.insert(id.clone()) {
            continue;
        }
        next.extend(
            edges
                .iter()
                .filter(|[from, _]| *from == id)
                .map(|[_, to]| to.clone()),
        );
    }
    kept
}

// ---- The plan ------------------------------------------------------------------------------------------------------

/// A planning stage's place: done, the current one, later, or waiting on the owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanState {
    Done,
    Current,
    Later,
    You,
}

/// One card of the Plan column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanCard {
    pub stage: &'static str,
    pub state: PlanState,
    pub title: String,
    pub line: String,
}

/// "brief" becomes "Brief".
pub fn stage_label(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The Plan column's five cards, each with what its stage holds.
pub fn plan_cards(p: &Project) -> Vec<PlanCard> {
    let at = stage_index(&p.stage);
    let kinds = |k: &[&str]| -> Vec<&Doc> {
        p.docs
            .iter()
            .filter(|d| k.contains(&d.kind.as_str()))
            .collect()
    };
    PLAN_STAGES
        .iter()
        .map(|&stage| {
            let i = stage_index(stage);
            let state = if i < at {
                PlanState::Done
            } else if i == at {
                PlanState::Current
            } else {
                PlanState::Later
            };
            let card = |title: String, line: String| PlanCard {
                stage,
                state,
                title,
                line,
            };
            match stage {
                "brief" => card(
                    "Brief".into(),
                    kinds(&["brief"])
                        .first()
                        .map_or("The orchestrator writes it".into(), |d| d.title.clone()),
                ),
                "research" => {
                    let n = kinds(&["research"]).len();
                    card(
                        "Research".into(),
                        match n {
                            0 => "Researchers attach what they find".into(),
                            1 => "1 finding".into(),
                            n => format!("{n} findings"),
                        },
                    )
                }
                "proposal" => {
                    let mut what: Vec<&str> = Vec::new();
                    for d in kinds(&["proposal", "design", "spec", "tasks", "revision"]) {
                        if !what.contains(&d.kind.as_str()) {
                            what.push(&d.kind);
                        }
                    }
                    card(
                        if p.revision > 1 {
                            format!("Proposal · rev {}", p.revision)
                        } else {
                            "Proposal".into()
                        },
                        if what.is_empty() {
                            "The planner writes it".into()
                        } else {
                            what.join(", ")
                        },
                    )
                }
                "critique" => {
                    let approve = p
                        .critiques
                        .iter()
                        .filter(|c| c.verdict == "approve")
                        .count();
                    let revise = p.critiques.iter().filter(|c| c.verdict == "revise").count();
                    let line = if p.critiques.is_empty() {
                        "Agents other than the author critique it".to_string()
                    } else if revise > 0 {
                        format!("{approve} approve · {revise} revise")
                    } else {
                        format!("{approve} approve")
                    };
                    card("Critique".into(), line)
                }
                _ => match &p.approved_by {
                    Some(by) => PlanCard {
                        stage,
                        state: PlanState::Done,
                        title: if by == "owner" {
                            "Approved by you".into()
                        } else {
                            format!("Approved by {by}")
                        },
                        line: "The tasks were posted from it".into(),
                    },
                    None => PlanCard {
                        stage,
                        state: if plan_waits_for_owner(p) {
                            PlanState::You
                        } else {
                            state
                        },
                        title: stage_label(stage),
                        line: if p.owner_approval {
                            "Waits for your approval".into()
                        } else {
                            "The orchestrator approves".into()
                        },
                    },
                },
            }
        })
        .collect()
}

// ---- The map ---------------------------------------------------------------------------------------------------------

/// A box on the map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The plan column's size: its cards stack from the top padding, `height` tall with the gaps between and after them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlanPart {
    pub width: f64,
    pub height: f64,
}

/// The map's padding above its first card and below its last (the web's `TOP`).
pub const MAP_TOP: f64 = 16.0;

/// Where the plan, the graph and delivery stand on the map. The links are drawn between their cards.
#[derive(Debug, Clone, PartialEq)]
pub struct MapGeometry {
    pub width: f64,
    pub height: f64,
    pub plan: Option<Area>,
    /// Where the layout's origin goes.
    pub graph: [f64; 2],
    pub delivery: Option<Area>,
}

/// The map as the web's `mapOf` sets it out: the plan and the graph start under the top padding, delivery is centred
/// on the map, and the map is as tall as its tallest part plus the padding; `gap` runs between the columns.
pub fn compose(
    layout: &apz_graph::Layout,
    plan: Option<PlanPart>,
    delivery: Option<(f64, f64)>,
    gap: f64,
) -> MapGeometry {
    let height = (plan.map_or(0.0, |p| p.height).max(MAP_TOP + layout.height)) + MAP_TOP;
    let graph = [plan.map_or(0.0, |p| p.width + gap), MAP_TOP];
    let plan_box = plan.map(|p| Area {
        x: 0.0,
        y: MAP_TOP,
        width: p.width,
        height: p.height,
    });
    let delivery_box = delivery.map(|(width, h)| Area {
        x: graph[0] + layout.width + gap,
        y: (height - h) / 2.0,
        width,
        height: h,
    });
    MapGeometry {
        width: delivery_box.map_or(graph[0] + layout.width, |d| d.x + d.width),
        height,
        plan: plan_box,
        graph,
        delivery: delivery_box,
    }
}

/// The state columns a project's tasks stand in when none of them has a link.
pub const STATE_COLUMNS: [(&str, &[WorkState]); 4] = [
    (
        "Open",
        &[WorkState::Ready, WorkState::Waiting, WorkState::Blocked],
    ),
    ("Working", &[WorkState::Working, WorkState::NeedsYou]),
    ("In review", &[WorkState::Review]),
    (
        "Ended",
        &[WorkState::Done, WorkState::Failed, WorkState::Withdrawn],
    ),
];

/// A graph built from the tasks outside projects: their links among themselves become its edges.
pub fn loose_graph(tasks: &[Task]) -> Graph {
    let loose: Vec<&Task> = tasks.iter().filter(|t| t.project.is_none()).collect();
    let ids: HashSet<&str> = loose.iter().map(|t| t.id.as_str()).collect();
    Graph {
        tasks: loose
            .iter()
            .map(|t| GraphTask {
                id: t.id.clone(),
                goal: t.goal.clone(),
                role: t.role.clone(),
                state: t.state.clone(),
                ready: t.ready,
                blocked_by: t.blocked_by.clone(),
                agent_cid: t.agent_cid,
                reviewer_cid: None,
            })
            .collect(),
        edges: loose
            .iter()
            .flat_map(|t| {
                t.after
                    .iter()
                    .filter(|id| ids.contains(id.as_str()))
                    .map(|id| [id.clone(), t.id.clone()])
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_000_000;

    fn node(id: &str) -> GraphTask {
        GraphTask {
            id: id.into(),
            goal: format!("Goal of {id}\nmore"),
            role: None,
            state: "open".into(),
            ready: false,
            blocked_by: Vec::new(),
            agent_cid: None,
            reviewer_cid: None,
        }
    }

    fn task_row(id: &str) -> Task {
        Task {
            id: id.into(),
            state: "open".into(),
            goal: id.into(),
            role: None,
            project: Some("pr1".into()),
            after: Vec::new(),
            ready: true,
            blocked_by: Vec::new(),
            agent_cid: None,
            note: None,
            outcome: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn project() -> Project {
        serde_json::from_value(serde_json::json!({
            "id": "pr1", "title": "Release", "goal": "g", "kind": "other", "stage": "implementation", "lead_cid": 1,
            "owner_approval": false, "repo": null,
            "docs": [{ "kind": "brief", "title": "The brief", "file": "f1", "repo_path": null, "commit": null,
                       "summary": "s", "by_cid": 1, "at_ms": 0 }],
            "critiques": [{ "by_cid": 3, "verdict": "approve", "text": "ok", "revision": 1, "at_ms": 0 }],
            "revision": 1, "approved_by": "Orchestrator", "tasks": ["t1", "t2"], "history": [], "created_ms": 0, "updated_ms": 0
        }))
        .expect("the hub's project reads")
    }

    fn step(stage: &str, tool: Option<&str>, since_ms: i64) -> Step {
        Step {
            stage: stage.into(),
            tool: tool.map(str::to_string),
            since_ms,
        }
    }

    fn edges(pairs: &[(&str, &str)]) -> Vec<[String; 2]> {
        pairs
            .iter()
            .map(|(a, b)| [a.to_string(), b.to_string()])
            .collect()
    }

    #[test]
    fn a_task_is_ready_waiting_blocked_working_or_needs_you() {
        let none: Vec<String> = Vec::new();
        let t1 = vec!["t1".to_string()];
        assert_eq!(WorkState::of("open", true, &none, None), WorkState::Ready);
        assert_eq!(
            WorkState::of("open", false, &none, None),
            WorkState::Waiting
        );
        assert_eq!(WorkState::of("open", false, &t1, None), WorkState::Blocked);
        let tool = step("tool", Some("bash"), 0);
        let asks = step("approval", Some("plaza_pay"), 0);
        assert_eq!(
            WorkState::of("claimed", false, &none, Some(&tool)),
            WorkState::Working
        );
        assert_eq!(
            WorkState::of("claimed", false, &none, Some(&asks)),
            WorkState::NeedsYou
        );
        assert_eq!(
            WorkState::of("awaiting_review", false, &none, None),
            WorkState::Review
        );
        let ended: Vec<WorkState> = ["done", "failed", "withdrawn"]
            .iter()
            .map(|s| WorkState::of(s, false, &none, None))
            .collect();
        assert_eq!(
            ended,
            [WorkState::Done, WorkState::Failed, WorkState::Withdrawn]
        );
        assert!(ended.iter().all(|s| s.ended()));
    }

    #[test]
    fn words_and_spans_read_as_the_web_says_them() {
        assert_eq!(listed(&["a".into()]), "a");
        assert_eq!(listed(&["a".into(), "b".into(), "c".into()]), "a, b and c");
        let spans: Vec<String> = [40_000, 12 * 60_000, 3 * 3_600_000, 2 * 86_400_000]
            .into_iter()
            .map(short_span)
            .collect();
        assert_eq!(spans, ["40s", "12m", "3h", "2d"]);
        assert_eq!(
            step_text(Some(&step("tool", Some("bash"), NOW - 120_000)), NOW),
            "Running bash · 2m"
        );
        assert_eq!(
            step_text(Some(&step("approval", Some("plaza_pay"), NOW)), NOW),
            "Waits for you to approve plaza_pay · 0s"
        );
        assert_eq!(step_text(None, NOW), "");
        assert_eq!(title_of("\n  First line \nsecond"), "First line");
    }

    #[test]
    fn a_card_says_who_has_the_task_or_what_it_waits_for() {
        let links = edges(&[("t1", "t3"), ("t2", "t3"), ("t3", "t4")]);
        let states: HashMap<&str, &str> = [
            ("t1", "done"),
            ("t2", "claimed"),
            ("t3", "open"),
            ("t4", "open"),
        ]
        .into();
        let state_of = |id: &str| states.get(id).map(|s| s.to_string());
        let task_name = |id: &str| format!("T{}", &id[1..]);
        let no_time = |_: &str| None;
        let label_of = |cid: u64| roles::agent_label(if cid == 7 { "qa" } else { "" }, cid);
        let thinking = |cid: u64| (cid == 7).then(|| step("thinking", None, NOW - 5_000));
        let ctx = LineContext {
            edges: &links,
            state_of: &state_of,
            task_name: &task_name,
            ready_at: &no_time,
            label_of: &label_of,
            step_of: &thinking,
            now: NOW,
        };
        assert_eq!(card_line(&node("t3"), WorkState::Waiting, &ctx), "After T2");
        let blocked = GraphTask {
            blocked_by: vec!["t1".into(), "t2".into()],
            ..node("t5")
        };
        assert_eq!(
            card_line(&blocked, WorkState::Blocked, &ctx),
            "Blocked: T1 and T2 did not finish"
        );
        let claimed = GraphTask {
            state: "claimed".into(),
            agent_cid: Some(7),
            ..node("t2")
        };
        assert_eq!(
            card_line(&claimed, WorkState::Working, &ctx),
            "QA #7 · Thinking · 5s"
        );
        let asks = |_: u64| Some(step("approval", Some("plaza_pay"), NOW - 120_000));
        let deciding = LineContext {
            step_of: &asks,
            ..ctx
        };
        assert_eq!(
            card_line(&claimed, WorkState::NeedsYou, &deciding),
            "Approve plaza_pay · QA #7 · 2m"
        );
        let qa = GraphTask {
            role: Some("qa".into()),
            ..node("t6")
        };
        assert_eq!(
            card_line(&qa, WorkState::Ready, &ctx),
            "Any QA agent can claim it"
        );
        let orchestrator = GraphTask {
            role: Some("orchestrator".into()),
            ..node("t6")
        };
        assert_eq!(
            card_line(&orchestrator, WorkState::Ready, &ctx),
            "Any Orchestrator agent can claim it"
        );
        assert_eq!(
            card_line(&node("t6"), WorkState::Ready, &ctx),
            "Any agent can claim it"
        );
        let five_minutes = |_: &str| Some(NOW - 300_000);
        let timed = LineContext {
            ready_at: &five_minutes,
            ..ctx
        };
        assert_eq!(card_line(&qa, WorkState::Ready, &timed), "Unclaimed for 5m");
        let in_review = GraphTask {
            state: "awaiting_review".into(),
            reviewer_cid: Some(9),
            ..node("t6")
        };
        assert_eq!(
            card_line(&in_review, WorkState::Review, &ctx),
            "Reviewed by agent #9"
        );
    }

    #[test]
    fn the_lens_keeps_the_decisions_and_the_work_after_them() {
        let links = edges(&[("t1", "t3"), ("t2", "t3"), ("t3", "t4")]);
        let states: HashMap<String, WorkState> = [
            ("t1", WorkState::Done),
            ("t2", WorkState::NeedsYou),
            ("t3", WorkState::Waiting),
            ("t4", WorkState::Waiting),
            ("t5", WorkState::Ready),
        ]
        .into_iter()
        .map(|(id, s)| (id.to_string(), s))
        .collect();
        let mut kept: Vec<String> = needs_me(&states, &links).into_iter().collect();
        kept.sort();
        assert_eq!(kept, ["t2", "t3", "t4"]);
        let calm: HashMap<String, WorkState> = [("t1".to_string(), WorkState::Done)].into();
        assert!(needs_me(&calm, &links).is_empty());
    }

    #[test]
    fn a_flow_line_has_the_plan_the_tasks_then_review_and_delivery() {
        let p = project();
        let no_steps = |_: u64| None;
        let t1 = Task {
            state: "done".into(),
            ..task_row("t1")
        };
        let t2 = Task {
            ready: false,
            ..task_row("t2")
        };
        let segs = flow_segments(Some(&p), &[&t1, &t2], &no_steps);
        let read: Vec<(SegKind, SegState)> = segs.iter().map(|s| (s.kind, s.state)).collect();
        let mut want = vec![(SegKind::Plan, SegState::Done); 5];
        want.extend([
            (SegKind::Task, SegState::Task(WorkState::Done)),
            (SegKind::Task, SegState::Task(WorkState::Waiting)),
            (SegKind::End, SegState::Later),
            (SegKind::End, SegState::Later),
        ]);
        assert_eq!(read, want);
        let loose = task_row("t9");
        let kinds: Vec<SegKind> = flow_segments(None, &[&loose], &no_steps)
            .iter()
            .map(|s| s.kind)
            .collect();
        assert_eq!(kinds, [SegKind::Task]);

        let asking = Project {
            stage: "critique".into(),
            approved_by: None,
            owner_approval: true,
            tasks: Vec::new(),
            ..project()
        };
        let states: Vec<SegState> = flow_segments(Some(&asking), &[], &no_steps)
            .iter()
            .map(|s| s.state)
            .collect();
        assert_eq!(
            states,
            [
                SegState::Done,
                SegState::Done,
                SegState::Done,
                SegState::Current,
                SegState::You,
                SegState::Later,
                SegState::Later
            ]
        );
        let orchestrated = Project {
            owner_approval: false,
            ..asking.clone()
        };
        assert_eq!(
            flow_segments(Some(&orchestrated), &[], &no_steps)[4].state,
            SegState::Later
        );

        let c1 = Task {
            state: "claimed".into(),
            ready: false,
            agent_cid: Some(7),
            ..task_row("t1")
        };
        let c2 = Task {
            state: "claimed".into(),
            ready: false,
            agent_cid: Some(8),
            ..task_row("t2")
        };
        let deciding = |cid: u64| {
            Some(if cid == 7 {
                step("approval", Some("plaza_pay"), 0)
            } else {
                step("tool", Some("bash"), 0)
            })
        };
        let task_states = |segs: Vec<Seg>| -> Vec<SegState> {
            segs.iter()
                .filter(|s| s.kind == SegKind::Task)
                .map(|s| s.state)
                .collect()
        };
        assert_eq!(
            task_states(flow_segments(Some(&p), &[&c1, &c2], &deciding)),
            [
                SegState::Task(WorkState::NeedsYou),
                SegState::Task(WorkState::Working)
            ]
        );
        assert_eq!(
            task_states(flow_segments(Some(&p), &[&c1, &c2], &no_steps)),
            [
                SegState::Task(WorkState::Working),
                SegState::Task(WorkState::Working)
            ]
        );
        assert_eq!(
            [
                waits_for_owner(Some(&asking), &[], &no_steps),
                waits_for_owner(Some(&p), &[&c1, &c2], &deciding),
                waits_for_owner(Some(&p), &[&c1, &c2], &no_steps),
                waits_for_owner(None, &[&c1, &c2], &deciding),
            ],
            [true, true, false, true]
        );
    }

    #[test]
    fn ready_tasks_nobody_claimed_wait_by_role() {
        let tasks = vec![
            Task {
                state: "done".into(),
                updated_at: 5_000,
                ..task_row("t1")
            },
            Task {
                role: Some("qa".into()),
                after: vec!["t1".into()],
                created_at: 1_000,
                ..task_row("t2")
            },
            Task {
                role: Some("qa".into()),
                created_at: 2_000,
                ..task_row("t3")
            },
            Task {
                created_at: 7_000,
                ..task_row("t4")
            },
            Task {
                role: Some("qa".into()),
                ready: false,
                ..task_row("t5")
            },
            Task {
                role: Some("qa".into()),
                state: "claimed".into(),
                ready: false,
                ..task_row("t6")
            },
        ];
        assert_eq!(
            waits_by_role(&tasks, 10_000),
            [
                RoleWait {
                    role: Some("qa".into()),
                    count: 2,
                    waited: 8_000
                },
                RoleWait {
                    role: None,
                    count: 1,
                    waited: 3_000
                }
            ]
        );
    }

    #[test]
    fn the_plan_cards_say_what_each_stage_holds() {
        let p = project();
        let cards = plan_cards(&p);
        assert!(cards.iter().all(|c| c.state == PlanState::Done));
        assert_eq!(cards[0].line, "The brief");
        assert_eq!(cards[4].title, "Approved by Orchestrator");
        let asking = Project {
            stage: "approval".into(),
            approved_by: None,
            owner_approval: true,
            ..project()
        };
        assert_eq!(plan_cards(&asking)[4].state, PlanState::You);
        assert_eq!(plan_cards(&asking)[4].line, "Waits for your approval");
    }

    #[test]
    fn the_map_holds_the_plan_the_graph_and_delivery() {
        let layout: apz_graph::Layout = serde_json::from_value(serde_json::json!({
            "width": 300, "height": 100, "columns": [["t1"], ["t2"]],
            "cards": [
                { "id": "t1", "column": 0, "row": 0, "x": 10, "y": 10, "width": 100, "height": 80 },
                { "id": "t2", "column": 1, "row": 0, "x": 160, "y": 10, "width": 100, "height": 80 }
            ],
            "routes": [{ "from": "t1", "to": "t2", "points": [[110, 50], [160, 50]] }]
        }))
        .expect("a layout reads");
        let plan = PlanPart {
            width: 50.0,
            height: 200.0,
        };
        let geo = compose(&layout, Some(plan), Some((40.0, 60.0)), 20.0);
        // The plan and the graph start under the top padding; the map is the tallest part plus the padding on both
        // sides; delivery is centred on it (the web's `mapOf`).
        assert_eq!((geo.width, geo.height), (430.0, 216.0));
        assert_eq!(geo.graph, [70.0, MAP_TOP]);
        assert_eq!(geo.plan.map(|a| (a.x, a.y)), Some((0.0, MAP_TOP)));
        assert_eq!(geo.delivery.map(|a| (a.x, a.y)), Some((390.0, 78.0)));
    }

    #[test]
    fn the_tasks_outside_projects_make_a_graph_of_their_own() {
        let tasks = vec![
            Task {
                project: None,
                ..task_row("t1")
            },
            Task {
                project: None,
                after: vec!["t1".into(), "t7".into()],
                ..task_row("t2")
            },
            task_row("t3"),
        ];
        let g = loose_graph(&tasks);
        let ids: Vec<&str> = g.tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["t1", "t2"]);
        assert_eq!(g.edges, edges(&[("t1", "t2")]));
    }
}
