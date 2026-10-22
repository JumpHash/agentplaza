//! The review graph (docs: the design's review-graph.md, R1–R13): a task's sealed revision in Weft reviewed by a
//! recipe, a graph of review steps that each run as a work item (R7).
//!
//! 1. **Start.** When a task's lane is sealed and the task awaits review, a run starts with the recipe for the
//!    kind of change (code, docs, research), its first rules applied.
//! 2. **Nodes.** A tool node runs in the hub (the change tree, divergence); an agent node is a work item for an
//!    agent of its role in the task's project, never the worker; a check node is a work item that runs commands
//!    in a fresh checkout and reports measured scores. Each starts once the nodes it runs after are done.
//! 3. **Scores and findings** come from the agents' tools (`review_tree`, `review_score`, `review_verdict`,
//!    `review_spawn`): levels −5…5 per axis and definition, with a confidence, a basis (measured or estimated) and
//!    a line of evidence (R4), findings anchored to the definition's lineage, and what each part touches.
//! 4. **Signed.** Each node's verdict is a `review` record in Weft signed with that reviewer's key; tools and the
//!    gate are signed by the hub; the owner's ruling by the owner (from the console).
//! 5. **Gate.** When every node is done, the rules decide (R3): a failed node or a rework rule sends the task back
//!    to its worker (`rework`); an owner rule raises the owner's card; else it passes, and the revision joins its
//!    project's landing queue. After the recipe's rounds, the owner decides (R11).

pub mod recipe;
pub mod routes;
pub mod tools;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::scheduler::Origin;
use crate::state::App;
use apz_proto::wire::WorkKind;
use recipe::{Condition, NodeDef, Recipe};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Running,
    Passed,
    Rework,
    Owner,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    Waiting,
    Running,
    Passed,
    Failed,
    Skipped,
}

impl NodeState {
    fn done(self) -> bool {
        matches!(
            self,
            NodeState::Passed | NodeState::Failed | NodeState::Skipped
        )
    }
}

/// One score: a definition (or file) on one axis.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Score {
    /// `path::qualified`, or a path.
    pub symbol: String,
    #[serde(default)]
    pub lineage: Option<String>,
    pub axis: String,
    /// −5 (much worse) … 5 (much better).
    pub level: i64,
    /// 0 … 1.
    #[serde(default = "half")]
    pub confidence: f64,
    /// `measured` or `estimated`.
    #[serde(default = "estimated")]
    pub basis: String,
    #[serde(default)]
    pub evidence: String,
    #[serde(default)]
    pub node: String,
}

fn half() -> f64 {
    0.5
}

fn estimated() -> String {
    "estimated".into()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Finding {
    pub symbol: String,
    #[serde(default)]
    pub lineage: Option<String>,
    pub text: String,
    /// `info`, `minor`, `major`, `blocker`.
    #[serde(default = "minor")]
    pub severity: String,
    #[serde(default)]
    pub node: String,
    /// The round it was found in; a finding follows its definition into later rounds.
    #[serde(default)]
    pub round: u32,
}

fn minor() -> String {
    "minor".into()
}

/// What a part touches, in a few words (built from Weft's references, worded by the node).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Touch {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub how: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeRun {
    pub def: NodeDef,
    #[serde(default)]
    pub parent: Option<String>,
    pub state: NodeState,
    #[serde(default)]
    pub assignee: Option<u64>,
    #[serde(default)]
    pub work: Option<u64>,
    #[serde(default)]
    pub tries: u32,
    #[serde(default)]
    pub scores: Vec<Score>,
    #[serde(default)]
    pub findings: Vec<Finding>,
    #[serde(default)]
    pub touches: Vec<Touch>,
    #[serde(default)]
    pub notes: String,
    /// The node's signed record in Weft (its log seq).
    #[serde(default)]
    pub record: Option<u64>,
    #[serde(default)]
    pub started_ms: u64,
    #[serde(default)]
    pub ended_ms: u64,
    #[serde(default)]
    pub spawned: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Outcome {
    /// `pass`, `rework`, `owner`, `fail`.
    pub decision: String,
    pub reasons: Vec<String>,
    #[serde(default)]
    pub replan: Vec<String>,
    pub at_ms: u64,
    #[serde(default)]
    pub by: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub task: String,
    pub project: String,
    pub worker: u64,
    pub change: String,
    pub revision: String,
    pub recipe: String,
    pub round: u32,
    pub rounds: u32,
    pub kind: String,
    pub state: RunState,
    pub nodes: Vec<NodeRun>,
    #[serde(default)]
    pub rules: Vec<recipe::Rule>,
    #[serde(default)]
    pub owner_asked: bool,
    #[serde(default)]
    pub outcome: Option<Outcome>,
    /// The change tree the nodes read (from Weft, at the start).
    #[serde(default)]
    pub tree: Value,
    #[serde(default)]
    pub summary: String,
    /// `review` (a sealed round), `recheck` (a landing candidate, `revision` is its digest) or `post-land` (main
    /// after the change landed).
    #[serde(default = "review_purpose")]
    pub purpose: String,
    /// For a re-check: the repo whose queue waits on it.
    #[serde(default)]
    pub repo: String,
    pub created_ms: u64,
    pub updated_ms: u64,
}

fn review_purpose() -> String {
    "review".into()
}

impl Run {
    pub fn node(&self, id: &str) -> Option<&NodeRun> {
        self.nodes.iter().find(|n| n.def.id == id)
    }

    fn node_mut(&mut self, id: &str) -> Option<&mut NodeRun> {
        self.nodes.iter_mut().find(|n| n.def.id == id)
    }

    pub fn scores(&self) -> impl Iterator<Item = &Score> {
        self.nodes.iter().flat_map(|n| n.scores.iter())
    }

    /// The lowest level per axis (measured scores outrank estimates on the same symbol and axis).
    pub fn lows(&self) -> BTreeMap<String, (i64, bool)> {
        let mut best: BTreeMap<(String, String), &Score> = BTreeMap::new();
        for s in self.scores() {
            let key = (s.symbol.clone(), s.axis.clone());
            match best.get(&key) {
                Some(b) if b.basis == "measured" && s.basis != "measured" => {}
                _ => {
                    best.insert(key, s);
                }
            }
        }
        let mut out: BTreeMap<String, (i64, bool)> = BTreeMap::new();
        for ((_, axis), s) in best {
            let measured = s.basis == "measured";
            let e = out.entry(axis).or_insert((i64::MAX, measured));
            if s.level < e.0 {
                *e = (s.level, measured);
            }
        }
        out
    }

    /// The four readings: the mean level of their axes' lows, rounded.
    pub fn readings(&self) -> Vec<(String, Option<i64>)> {
        let lows = self.lows();
        recipe::READINGS
            .iter()
            .map(|(name, axes)| {
                let v: Vec<i64> = axes
                    .iter()
                    .filter_map(|a| lows.get(*a).map(|l| l.0))
                    .collect();
                (
                    name.to_string(),
                    (!v.is_empty())
                        .then(|| (v.iter().sum::<i64>() as f64 / v.len() as f64).round() as i64),
                )
            })
            .collect()
    }

    /// The run in one sentence, for the owner's card and the baseline's verdict strip.
    pub fn sentence(&self) -> String {
        let lows = self.lows();
        let worse: Vec<String> = lows
            .iter()
            .filter(|(_, (l, _))| *l <= -2)
            .map(|(a, (l, _))| format!("{} {l}", recipe::axis_name(a)))
            .collect();
        let failed: Vec<&str> = self
            .nodes
            .iter()
            .filter(|n| n.state == NodeState::Failed)
            .map(|n| n.def.id.as_str())
            .collect();
        match (&self.outcome, failed.is_empty(), worse.is_empty()) {
            (Some(o), _, _) if o.decision == "pass" => "Every review step passed.".into(),
            (_, false, _) => format!(
                "{} did not pass{}.",
                failed.join(", "),
                if worse.is_empty() {
                    String::new()
                } else {
                    format!("; worse on {}", worse.join(", "))
                }
            ),
            (_, true, false) => format!("Worse on {}.", worse.join(", ")),
            _ => "No step found anything against it.".into(),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Book {
    runs: BTreeMap<String, Run>,
    next: u64,
    /// The owner's recipes and versions (the built-in ones are always there).
    recipes: Vec<Recipe>,
    /// Proposals from the orchestrator, waiting for the owner.
    proposals: Vec<Proposal>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    pub by: u64,
    pub recipe: Recipe,
    pub why: String,
    pub state: String,
    pub at_ms: u64,
}

/// The review graph's runs and recipes, saved at `<state>/review-graph.json`.
pub struct Reviews {
    book: Mutex<Book>,
    path: Option<PathBuf>,
}

impl Default for Reviews {
    fn default() -> Self {
        Reviews {
            book: Mutex::new(Book::default()),
            path: None,
        }
    }
}

impl Reviews {
    pub fn load(state_dir: &std::path::Path) -> Reviews {
        let path = state_dir.join("review-graph.json");
        let book = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Reviews {
            book: Mutex::new(book),
            path: Some(path),
        }
    }

    fn save(&self, book: &Book) {
        if let Some(p) = &self.path {
            let _ = crate::state::write_atomic(p, &serde_json::to_string(book).unwrap_or_default());
        }
    }

    fn with<T>(&self, f: impl FnOnce(&mut Book) -> T) -> T {
        let mut book = self.book.lock().unwrap_or_else(|p| p.into_inner());
        let out = f(&mut book);
        self.save(&book);
        out
    }

    pub fn run(&self, id: &str) -> Option<Run> {
        self.book.lock().ok()?.runs.get(id).cloned()
    }

    pub fn runs_of(&self, task: &str) -> Vec<Run> {
        let book = self.book.lock().unwrap_or_else(|p| p.into_inner());
        let mut v: Vec<Run> = book
            .runs
            .values()
            .filter(|r| r.task == task)
            .cloned()
            .collect();
        v.sort_by_key(|r| std::cmp::Reverse(r.created_ms));
        v
    }

    pub fn active(&self, task: &str) -> Option<Run> {
        self.book
            .lock()
            .ok()?
            .runs
            .values()
            .find(|r| r.task == task && matches!(r.state, RunState::Running | RunState::Owner))
            .cloned()
    }

    /// Every recipe version: the owner's, then the built-in ones they have not replaced.
    pub fn recipes(&self) -> Vec<Recipe> {
        let book = self.book.lock().unwrap_or_else(|p| p.into_inner());
        let mut all = recipe::built_in();
        all.extend(book.recipes.iter().cloned());
        all.sort_by(|a, b| a.id.cmp(&b.id).then(a.version.cmp(&b.version)));
        all.dedup_by(|a, b| a.id == b.id && a.version == b.version);
        all
    }

    /// A recipe's latest version (or the one named).
    pub fn recipe(&self, id: &str, version: Option<u32>) -> Option<Recipe> {
        self.recipes()
            .into_iter()
            .filter(|r| r.id == id && version.is_none_or(|v| r.version == v))
            .max_by_key(|r| r.version)
    }

    /// A new version of a recipe (the owner's).
    pub fn put_recipe(&self, r: Recipe) -> Result<Recipe, String> {
        r.check()?;
        if self
            .recipes()
            .iter()
            .any(|x| x.id == r.id && x.version >= r.version)
        {
            return Err(format!(
                "{} is already at version {} or later",
                r.id, r.version
            ));
        }
        self.with(|b| b.recipes.push(r.clone()));
        Ok(r)
    }

    pub fn proposals(&self) -> Vec<Proposal> {
        self.book
            .lock()
            .map(|b| b.proposals.clone())
            .unwrap_or_default()
    }

    pub fn propose(&self, by: u64, r: Recipe, why: &str) -> Result<Proposal, String> {
        r.check()?;
        Ok(self.with(|b| {
            b.next += 1;
            let p = Proposal {
                id: format!("rp{}", b.next),
                by,
                recipe: r,
                why: why.into(),
                state: "pending".into(),
                at_ms: apz_plaza::now_ms(),
            };
            b.proposals.push(p.clone());
            p
        }))
    }

    pub fn decide_proposal(&self, id: &str, approve: bool) -> Result<Proposal, String> {
        let p = self.with(|b| {
            let p = b
                .proposals
                .iter_mut()
                .find(|p| p.id == id && p.state == "pending")
                .ok_or_else(|| format!("no pending proposal {id}"))?;
            p.state = if approve {
                "approved".into()
            } else {
                "declined".into()
            };
            Ok::<Proposal, String>(p.clone())
        })?;
        if approve {
            self.put_recipe(p.recipe.clone())?;
        }
        Ok(p)
    }

    /// Takes waiting node `node` of running review `run` for one caller: it moves to running under the book's lock.
    /// Ticks run side by side (each verdict ticks, and so does the review tick), and two that both saw a node ready
    /// would otherwise both start it: a step sent twice, or a gate deciding twice.
    fn claim(&self, run: &str, node: &str) -> bool {
        self.with(|b| {
            let Some(r) = b.runs.get_mut(run) else {
                return false;
            };
            if r.state != RunState::Running {
                return false;
            }
            match r.node_mut(node) {
                Some(n) if n.state == NodeState::Waiting => {
                    n.state = NodeState::Running;
                    r.updated_ms = apz_plaza::now_ms();
                    true
                }
                _ => false,
            }
        })
    }

    fn update(&self, id: &str, f: impl FnOnce(&mut Run)) -> Option<Run> {
        self.with(|b| {
            let r = b.runs.get_mut(id)?;
            f(r);
            r.updated_ms = apz_plaza::now_ms();
            Some(r.clone())
        })
    }

    /// The node an agent is working on: its run and node id.
    pub fn assigned(&self, cid: u64) -> Option<(String, String)> {
        let book = self.book.lock().ok()?;
        for r in book.runs.values().filter(|r| r.state == RunState::Running) {
            for n in &r.nodes {
                if n.assignee == Some(cid) && n.state == NodeState::Running {
                    return Some((r.id.clone(), n.def.id.clone()));
                }
            }
        }
        None
    }

    /// The step a tool call is about: the one whose work item the call came in, else the agent's first running step.
    pub fn step_for(&self, cid: u64, work: u64) -> Option<(String, String)> {
        self.by_work(work)
            .filter(|(r, n)| {
                self.run(r)
                    .and_then(|r| {
                        r.node(n)
                            .map(|n| n.assignee == Some(cid) && n.state == NodeState::Running)
                    })
                    .unwrap_or(false)
            })
            .or_else(|| self.assigned(cid))
    }

    fn by_work(&self, work: u64) -> Option<(String, String)> {
        let book = self.book.lock().ok()?;
        for r in book.runs.values() {
            for n in &r.nodes {
                if n.work == Some(work) {
                    return Some((r.id.clone(), n.def.id.clone()));
                }
            }
        }
        None
    }
}

// ── starting a run ───────────────────────────────────────────────────────────────────────────────────────────────

/// Starts a run for a task awaiting review whose lane is sealed; `None` when it has no sealed revision in Weft
/// (the task is reviewed the old way), or a run is already going.
pub fn start(app: &App, task: &str) -> Option<String> {
    if app.reviews.active(task).is_some() {
        return None;
    }
    let record = app.plaza.tasks.find(task)?;
    let view = record.view();
    let worker = view.agent_cid?;
    let project = record.project()?.to_string();
    let change = app.weft.lane_of(task)?;
    let detail = app.weft.get(&format!("/changes/{change}")).ok()?;
    let revision = detail["rounds"].as_array()?.last()?.as_str()?.to_string();
    // One run per sealed round.
    if app
        .reviews
        .runs_of(task)
        .iter()
        .any(|r| r.revision == revision)
    {
        return None;
    }
    let tree = app.weft.get(&format!("/revisions/{revision}/tree")).ok()?;
    let project_kind = app
        .projects
        .get(&project)
        .map(|p| p.kind)
        .unwrap_or_default();
    let kind = recipe::kind_of(&tree, &project_kind).to_string();
    let recipe = pick_recipe(app, &kind, record.role().unwrap_or(""))?;
    let (nodes, owner) = plan(&recipe, &tree);
    let now = apz_plaza::now_ms();
    // The checks above ran unlocked, and the scheduler (`handles`) and the review tick both start runs: they are made
    // again under the book's lock, which also numbers the round, so two callers cannot start the same round twice.
    let (id, round) = app.reviews.with(|b| {
        let of_task = || b.runs.values().filter(|r| r.task == task);
        if of_task().any(|r| {
            r.revision == revision || matches!(r.state, RunState::Running | RunState::Owner)
        }) {
            return None;
        }
        let round = of_task().count() as u32 + 1;
        b.next += 1;
        let id = format!("rv{}", b.next);
        b.runs.insert(
            id.clone(),
            Run {
                id: id.clone(),
                task: task.into(),
                project,
                worker,
                change,
                revision,
                recipe: recipe.key(),
                round,
                rounds: recipe.rounds,
                kind,
                state: RunState::Running,
                nodes,
                rules: recipe.rules.clone(),
                owner_asked: owner,
                outcome: None,
                tree,
                summary: view.note.unwrap_or_default(),
                purpose: review_purpose(),
                repo: String::new(),
                created_ms: now,
                updated_ms: now,
            },
        );
        Some((id, round))
    })?;
    app.bus.publish(
        "review_run",
        &json!({ "run": id, "task": task, "state": "running", "round": round }),
    );
    Some(id)
}

fn pick_recipe(app: &App, kind: &str, role: &str) -> Option<Recipe> {
    let all = app.reviews.recipes();
    let latest = |pred: &dyn Fn(&Recipe) -> bool| {
        all.iter()
            .filter(|r| pred(r))
            .max_by_key(|r| r.version)
            .cloned()
    };
    latest(&|r| {
        r.applies.kinds.iter().any(|k| k == kind) && r.applies.roles.iter().any(|x| x == role)
    })
    .or_else(|| {
        latest(&|r| r.applies.kinds.iter().any(|k| k == kind) && r.applies.roles.is_empty())
    })
    .or_else(|| latest(&|r| r.id == "code"))
}

/// The recipe's nodes for this change, its first rules applied; and whether a rule asks for the owner.
fn plan(recipe: &Recipe, tree: &Value) -> (Vec<NodeRun>, bool) {
    let files: Vec<&str> = tree["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f["path"].as_str())
        .collect();
    let is_test = |p: &str| {
        p.contains("/tests/")
            || p.starts_with("tests/")
            || p.contains("_test.")
            || p.contains(".test.")
            || p.ends_with("_tests.rs")
            || p.contains("/test/")
    };
    let is_doc = |p: &str| p.ends_with(".md") || p.starts_with("docs/");
    // Comment-only: every changed definition kept its shape (only comments or whitespace moved): Weft reports no
    // symbol changes for such files beyond `(top level)`.
    let comment_only = !files.is_empty()
        && tree["files"].as_array().into_iter().flatten().all(|f| {
            f["symbols"]
                .as_array()
                .is_none_or(|s| s.iter().all(|x| x["qualified"] == "(top level)"))
                && f["pack"].is_string()
        });
    let mut keep: Option<Vec<String>> = None;
    let mut extra: Vec<NodeDef> = Vec::new();
    let mut owner = false;
    for rule in &recipe.first {
        let hit = match rule.when.as_str() {
            "comment-only" => comment_only,
            "test-only" => !files.is_empty() && files.iter().all(|p| is_test(p)),
            "docs-only" => !files.is_empty() && files.iter().all(|p| is_doc(p)),
            w => match w.strip_prefix("touches:") {
                Some(globs) => files
                    .iter()
                    .any(|p| globs.split(',').any(|g| weft::derived::glob(g.trim(), p))),
                None => false,
            },
        };
        if !hit {
            continue;
        }
        if !rule.only.is_empty() && keep.is_none() {
            keep = Some(rule.only.clone());
        }
        for a in &rule.add {
            if a == "owner" {
                owner = true;
            } else if let Some(rest) = a.strip_prefix("role:") {
                let (role, axes) = rest.split_once(':').unwrap_or((rest, ""));
                extra.push(NodeDef {
                    id: format!("extra-{role}"),
                    kind: "agent".into(),
                    role: Some(role.into()),
                    axes: axes
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect(),
                    after: vec!["tree".into()],
                    ..Default::default()
                });
            }
        }
    }
    let mut nodes: Vec<NodeRun> = Vec::new();
    for d in recipe.nodes.iter().cloned().chain(extra) {
        let skipped = d.kind != "gate"
            && keep.as_ref().is_some_and(|k| !k.contains(&d.id))
            && !d.id.starts_with("extra-");
        let mut def = d;
        if def.kind == "gate" {
            // The gate waits for every node, the added ones too.
            def.after = recipe
                .nodes
                .iter()
                .filter(|n| n.kind != "gate")
                .map(|n| n.id.clone())
                .collect();
            if owner
                || recipe
                    .first
                    .iter()
                    .any(|r| r.add.iter().any(|a| a.starts_with("role:")))
            {
                for r in &recipe.first {
                    for a in &r.add {
                        if let Some(rest) = a.strip_prefix("role:") {
                            let role = rest.split(':').next().unwrap_or(rest);
                            let id = format!("extra-{role}");
                            if !def.after.contains(&id) {
                                def.after.push(id);
                            }
                        }
                    }
                }
            }
        }
        nodes.push(NodeRun {
            def,
            parent: None,
            state: if skipped {
                NodeState::Skipped
            } else {
                NodeState::Waiting
            },
            assignee: None,
            work: None,
            tries: 0,
            scores: vec![],
            findings: vec![],
            touches: vec![],
            notes: if skipped {
                "skipped by a first rule".into()
            } else {
                String::new()
            },
            record: None,
            started_ms: 0,
            ended_ms: 0,
            spawned: 0,
        });
    }
    // Added nodes only exist when their first rule hit.
    let present: Vec<String> = nodes.iter().map(|n| n.def.id.clone()).collect();
    for n in nodes.iter_mut().filter(|n| n.def.kind == "gate") {
        n.def.after.retain(|a| present.contains(a));
    }
    (nodes, owner)
}

/// Whether the review graph reviews this task: its lane is sealed in Weft, so it has a run, or gets one now.
pub fn handles(app: &App, task: &str) -> bool {
    app.reviews.active(task).is_some()
        || start(app, task).is_some()
        || (app.weft.lane_of(task).is_some() && !app.reviews.runs_of(task).is_empty())
}

// ── advancing runs ───────────────────────────────────────────────────────────────────────────────────────────────

/// One pass over the running reviews: start the tasks' runs, start ready nodes, close the gates.
pub fn tick(app: &App) {
    if app.control.paused() || !app.weft.running() {
        return;
    }
    // Tasks awaiting review with a sealed lane get a run, except the work loop's (their reviewer is a seat).
    for task in app.plaza.tasks.list() {
        if task.state() == apz_plaza::tasks::TaskState::AwaitingReview.as_str()
            && !crate::work_loop::owns(app, task.id())
        {
            start(app, task.id());
        }
    }
    let running: Vec<Run> = app
        .reviews
        .book
        .lock()
        .map(|b| {
            b.runs
                .values()
                .filter(|r| r.state == RunState::Running)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    for run in running {
        for node in &run.nodes {
            if node.state != NodeState::Waiting || node.def.kind == "gate" {
                continue;
            }
            let ready = node
                .def
                .after
                .iter()
                .all(|a| run.node(a).is_none_or(|n| n.state.done()));
            if ready {
                start_node(app, &run, &node.def.id);
            }
        }
        // The gate, once every other node is done.
        if let Some(run) = app.reviews.run(&run.id)
            && run
                .nodes
                .iter()
                .filter(|n| n.def.kind != "gate")
                .all(|n| n.state.done())
        {
            gate(app, &run.id);
        }
    }
}

fn start_node(app: &App, run: &Run, node: &str) {
    let Some(n) = run.node(node) else { return };
    if !app.reviews.claim(&run.id, node) {
        return;
    }
    match n.def.kind.as_str() {
        "tool" => {
            let (scores, notes) = match n.def.tool.as_deref() {
                Some("divergence") => divergence_scores(&run.tree),
                _ => (
                    vec![],
                    format!(
                        "{} files, +{} −{}",
                        run.tree["files"].as_array().map(|f| f.len()).unwrap_or(0),
                        run.tree["add"],
                        run.tree["del"]
                    ),
                ),
            };
            let record = app.weft.hub("attest", json!({ "revision": run.revision, "digest": revision_digest(app, &run.revision), "predicate_type": "checked", "predicate": { "run": run.id, "node": node, "tool": n.def.tool, "scores": scores } })).ok().and_then(|v| v["seq"].as_u64());
            app.reviews.update(&run.id, |r| {
                if let Some(n) = r.node_mut(node) {
                    n.state = NodeState::Passed;
                    n.scores = scores;
                    n.notes = notes;
                    n.record = record;
                    n.started_ms = apz_plaza::now_ms();
                    n.ended_ms = n.started_ms;
                }
            });
        }
        "owner" => {
            app.reviews.update(&run.id, |r| {
                r.owner_asked = true;
                if let Some(n) = r.node_mut(node) {
                    n.state = NodeState::Passed;
                    n.notes = "the owner decides at the gate".into();
                }
            });
        }
        "agent" | "check" => {
            let role = n
                .def
                .role
                .clone()
                .unwrap_or_else(|| crate::scheduler::REVIEWER_ROLE.into());
            let taken: Vec<u64> = run.nodes.iter().filter_map(|x| x.assignee).collect();
            let Some(cid) = pick_agent(app, &run.project, &role, run.worker, &taken) else {
                // No agent of that role: a reviewer takes it, else it waits for the next tick (the timeout raises the
                // owner's card).
                if role != crate::scheduler::REVIEWER_ROLE
                    && let Some(cid) = pick_agent(
                        app,
                        &run.project,
                        crate::scheduler::REVIEWER_ROLE,
                        run.worker,
                        &taken,
                    )
                {
                    assign(app, run, node, cid);
                } else {
                    unclaim(app, &run.id, node);
                }
                return;
            };
            assign(app, run, node, cid);
        }
        _ => unclaim(app, &run.id, node),
    }
}

/// Puts a node [`Reviews::claim`] took back to waiting, for a later tick: nothing could start it now.
fn unclaim(app: &App, run: &str, node: &str) {
    app.reviews.update(run, |r| {
        if let Some(n) = r.node_mut(node)
            && n.state == NodeState::Running
            && n.assignee.is_none()
        {
            n.state = NodeState::Waiting;
        }
    });
}

/// An agent of `role` working on `project`, not the worker, preferring one not already on this run and idle.
fn pick_agent(app: &App, project: &str, role: &str, worker: u64, taken: &[u64]) -> Option<u64> {
    let on_project = crate::projects::agents_of(app, project);
    let mut candidates = app.agents.select(|a| {
        (a.role == role
            && a.cid != worker
            && on_project.contains(&a.cid)
            && a.status != crate::agents::Status::Offline
            && a.kind != "model")
            .then(|| (a.cid, a.status == crate::agents::Status::Idle))
    });
    candidates.sort_by_key(|(cid, idle)| (taken.contains(cid), !idle, *cid));
    candidates.first().map(|c| c.0)
}

fn assign(app: &App, run: &Run, node: &str, cid: u64) {
    let Some(n) = run.node(node) else { return };
    let prompt = node_prompt(app, run, n);
    let origin = Origin {
        task: None,
        ..Origin::default()
    };
    // The step is the agent's before its work goes out: the agent can call review_tree before enqueue returns, and
    // until the work id is recorded `step_for` finds the step by its running assignee.
    app.reviews.update(&run.id, |r| {
        if let Some(n) = r.node_mut(node) {
            n.state = NodeState::Running;
            n.assignee = Some(cid);
            n.tries += 1;
            n.started_ms = apz_plaza::now_ms();
        }
    });
    let work = app.work.enqueue(
        app,
        cid,
        WorkKind::Task,
        format!("review-{}-{node}", run.id),
        prompt,
        origin,
    );
    app.reviews.update(&run.id, |r| {
        if let Some(n) = r.node_mut(node) {
            n.work = Some(work);
        }
    });
    app.bus.publish(
        "review_node",
        &json!({ "run": run.id, "node": node, "state": "running", "cid": cid }),
    );
}

fn node_prompt(app: &App, run: &Run, n: &NodeRun) -> String {
    let record = app.plaza.tasks.find(&run.task);
    let goal = record
        .as_ref()
        .map(|t| t.goal().to_string())
        .unwrap_or_default();
    let acceptance = record
        .as_ref()
        .and_then(|t| t.acceptance().map(str::to_string))
        .unwrap_or_else(|| "none stated".into());
    let worker = app
        .agents
        .get(run.worker)
        .map(|a| a.label())
        .unwrap_or_else(|| format!("agent {}", run.worker));
    let axes: Vec<String> = n
        .def
        .axes
        .iter()
        .map(|a| format!("{a} ({})", recipe::axis_name(a)))
        .collect();
    let mut p = format!(
        "Review step `{node}` of review {run} (round {round} of task {task}: {goal}).\nAcceptance: {acceptance}\nThe worker ({worker}) reported: {summary}\nThe change is {change} in Weft ({rev}).\n",
        node = n.def.id,
        run = run.id,
        round = run.round,
        task = run.task,
        change = run.change,
        rev = run.revision,
        summary = run.summary
    );
    if !n.def.ask.is_empty() {
        p.push_str(&format!("{}\n", n.def.ask));
    }
    if n.def.kind == "check" {
        let checkout = match run.purpose.as_str() {
            "recheck" => format!(
                "weft checkout --candidate {} --dir /tmp/{}",
                run.revision, run.id
            ),
            _ => format!("weft checkout {} --dir /tmp/{}", run.change, run.id),
        };
        p.push_str(&format!(
            "Run these in a fresh checkout ({}), each from its root:\n{}\nReport each with review_score (basis measured, axes {}), then review_verdict: pass only when every command succeeded.\n",
            checkout,
            n.def.commands.iter().map(|c| format!("  - {c}")).collect::<Vec<_>>().join("\n"),
            if axes.is_empty() { "tst".into() } else { axes.join(", ") }
        ));
    } else {
        if n.def.constraints {
            p.push_str("Read the project's invariants (its brief and its design documents: plaza_projects, then the files) and fail this step if the change breaks one; name which and where.\n");
        }
        if !axes.is_empty() {
            p.push_str(&format!("Your axes: {}.\n", axes.join(", ")));
        }
        p.push_str("1. review_tree: the change by intent, each definition with what it touches, who wrote what, and the diff.\n");
        p.push_str("2. review_score: for each definition you judge, a level from -5 (much worse) to 5 (much better) on each of your axes, your confidence, the basis (measured when you ran something, else estimated) and one line of evidence; add findings, and say in a few words how parts touch the rest.\n");
        if n.def.sub_reviews > 0 {
            p.push_str(&format!("3. When a measurement would settle an estimate, review_spawn adds a sub-review (up to {}).\n", n.def.sub_reviews));
        }
        p.push_str("Finish with review_verdict: pass or fail for this step, with short notes. Judge the change, not the worker.\n");
    }
    p
}

fn revision_digest(app: &App, rev: &str) -> Value {
    app.weft
        .get(&format!("/revisions/{rev}"))
        .map(|r| r["digest"].clone())
        .unwrap_or(Value::Null)
}

/// Divergence scores from the tree: an open lane changing the same definition is −3, the same file −1.
fn divergence_scores(tree: &Value) -> (Vec<Score>, String) {
    let mut scores = Vec::new();
    let mut notes = Vec::new();
    for d in tree["divergence"].as_array().into_iter().flatten() {
        let other = d["change"].as_str().unwrap_or("");
        let task = d["task"].as_str().unwrap_or("-");
        for s in d["symbols"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s.as_str())
        {
            scores.push(Score {
                symbol: s.into(),
                lineage: None,
                axis: "div".into(),
                level: -3,
                confidence: 0.9,
                basis: "measured".into(),
                evidence: format!("{other} ({task}) changes it too"),
                node: "divergence".into(),
            });
        }
        for f in d["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| f.as_str())
        {
            if !scores
                .iter()
                .any(|s: &Score| s.symbol.starts_with(&format!("{f}::")))
            {
                scores.push(Score {
                    symbol: f.into(),
                    lineage: None,
                    axis: "div".into(),
                    level: -1,
                    confidence: 0.9,
                    basis: "measured".into(),
                    evidence: format!("{other} ({task}) changes this file"),
                    node: "divergence".into(),
                });
            }
        }
        notes.push(format!("{other} ({task})"));
    }
    let n = if notes.is_empty() {
        "no other open lane changes the same files".into()
    } else {
        format!("shares files with {}", notes.join(", "))
    };
    (scores, n)
}

// ── what agents report ───────────────────────────────────────────────────────────────────────────────────────────

/// Scores, findings and touches from the agent on a node.
pub fn report(
    app: &App,
    cid: u64,
    work: u64,
    scores: Vec<Score>,
    findings: Vec<Finding>,
    touches: Vec<Touch>,
) -> Result<String, String> {
    let (run, node) = app
        .reviews
        .step_for(cid, work)
        .ok_or("you have no review step to report on")?;
    let r = app.reviews.run(&run).ok_or("no such review")?;
    let axes = r
        .node(&node)
        .map(|n| n.def.axes.clone())
        .unwrap_or_default();
    for s in &scores {
        if !(-5..=5).contains(&s.level) {
            return Err(format!("{}: a level is between -5 and 5", s.symbol));
        }
        if !axes.is_empty() && !axes.contains(&s.axis) && s.axis != "kind" {
            return Err(format!(
                "{} is not one of this step's axes ({})",
                s.axis,
                axes.join(", ")
            ));
        }
    }
    let lineage = |sym: &str| lineage_of(&r.tree, sym);
    let n_scores = scores.len();
    app.reviews.update(&run, |r| {
        let round = r.round;
        if let Some(n) = r.node_mut(&node) {
            for mut s in scores {
                s.node = node.clone();
                s.lineage = s.lineage.or_else(|| lineage(&s.symbol));
                n.scores
                    .retain(|x| !(x.symbol == s.symbol && x.axis == s.axis));
                n.scores.push(s);
            }
            for mut f in findings {
                f.node = node.clone();
                f.round = round;
                f.lineage = f.lineage.or_else(|| lineage(&f.symbol));
                n.findings.push(f);
            }
            n.touches.extend(touches);
        }
    });
    app.bus.publish(
        "review_node",
        &json!({ "run": run, "node": node, "state": "scored" }),
    );
    Ok(format!("{n_scores} scores recorded on {node}"))
}

fn lineage_of(tree: &Value, symbol: &str) -> Option<String> {
    for f in tree["files"].as_array()? {
        for s in f["symbols"].as_array().into_iter().flatten() {
            if s["id"].as_str() == Some(symbol)
                || s["qualified"].as_str().is_some_and(|q| symbol.ends_with(q))
            {
                return s["lineage"].as_str().map(str::to_string);
            }
        }
    }
    None
}

/// The agent's verdict on its node, signed with its key in Weft.
pub fn verdict(app: &App, cid: u64, work: u64, pass: bool, notes: &str) -> Result<String, String> {
    let (run, node) = app
        .reviews
        .step_for(cid, work)
        .ok_or("you have no review step to give a verdict on")?;
    let r = app.reviews.run(&run).ok_or("no such review")?;
    let n = r.node(&node).ok_or("no such step")?;
    let scores_digest = weft::hash::Hash::of(
        serde_json::to_string(&n.scores)
            .unwrap_or_default()
            .as_bytes(),
    );
    let signed = crate::weft_hub::sign_as_agent(
        app,
        cid,
        "attest",
        json!({ "revision": r.revision, "digest": revision_digest(app, &r.revision), "predicate_type": "review", "predicate": { "run": run, "node": node, "pass": pass, "notes": notes, "axes": n.def.axes, "scores": scores_digest, "round": r.round } }),
    );
    let record = signed.as_ref().ok().and_then(|v| v["seq"].as_u64());
    if let Err(e) = &signed {
        eprintln!("apid: the verdict on {run}/{node} was not signed in Weft: {e}");
    }
    app.reviews.update(&run, |r| {
        if let Some(n) = r.node_mut(&node) {
            n.state = if pass {
                NodeState::Passed
            } else {
                NodeState::Failed
            };
            n.notes = notes.into();
            n.record = record;
            n.ended_ms = apz_plaza::now_ms();
        }
    });
    app.bus.publish(
        "review_node",
        &json!({ "run": run, "node": node, "state": if pass { "passed" } else { "failed" } }),
    );
    tick(app);
    Ok(format!(
        "{node}: {}",
        if pass { "passed" } else { "failed" }
    ))
}

/// A sub-review the agent on a node adds, within its node's limit.
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    app: &App,
    cid: u64,
    work: u64,
    kind: &str,
    role: Option<String>,
    axes: Vec<String>,
    commands: Vec<String>,
    ask: &str,
) -> Result<String, String> {
    let (run, node) = app
        .reviews
        .step_for(cid, work)
        .ok_or("you have no review step to add to")?;
    let r = app.reviews.run(&run).ok_or("no such review")?;
    let n = r.node(&node).ok_or("no such step")?;
    if n.spawned >= n.def.sub_reviews {
        return Err(format!(
            "{node} may add {} sub-reviews; it has",
            n.def.sub_reviews
        ));
    }
    if !matches!(kind, "agent" | "check") {
        return Err("a sub-review is an agent or a check".into());
    }
    if kind == "check" && commands.is_empty() {
        return Err("a check names the commands it runs".into());
    }
    let id = format!("{node}.{}", n.spawned + 1);
    let def = NodeDef {
        id: id.clone(),
        kind: kind.into(),
        role: Some(role.unwrap_or_else(|| {
            if kind == "check" {
                "qa".into()
            } else {
                crate::scheduler::REVIEWER_ROLE.into()
            }
        })),
        axes,
        commands,
        after: vec![],
        ask: ask.into(),
        ..Default::default()
    };
    app.reviews.update(&run, |r| {
        if let Some(p) = r.node_mut(&node) {
            p.spawned += 1;
        }
        r.nodes.push(NodeRun {
            def,
            parent: Some(node.clone()),
            state: NodeState::Waiting,
            assignee: None,
            work: None,
            tries: 0,
            scores: vec![],
            findings: vec![],
            touches: vec![],
            notes: String::new(),
            record: None,
            started_ms: 0,
            ended_ms: 0,
            spawned: 0,
        });
        for g in r.nodes.iter_mut().filter(|n| n.def.kind == "gate") {
            g.def.after.push(id.clone());
        }
    });
    tick(app);
    Ok(format!("{id} added; the gate waits for it"))
}

/// A node's work item ended. Without a verdict it is given to another agent once, then the step fails.
pub fn work_ended(app: &App, work: u64) {
    let Some((run, node)) = app.reviews.by_work(work) else {
        return;
    };
    let Some(r) = app.reviews.run(&run) else {
        return;
    };
    let Some(n) = r.node(&node) else { return };
    if n.state != NodeState::Running {
        return;
    }
    if n.tries < 2 {
        let taken: Vec<u64> = r.nodes.iter().filter_map(|x| x.assignee).collect();
        let role = n
            .def
            .role
            .clone()
            .unwrap_or_else(|| crate::scheduler::REVIEWER_ROLE.into());
        if let Some(cid) = pick_agent(app, &r.project, &role, r.worker, &taken)
            .or_else(|| pick_agent(app, &r.project, &role, r.worker, &[]))
        {
            assign(app, &r, &node, cid);
            return;
        }
    }
    app.reviews.update(&run, |r| {
        if let Some(n) = r.node_mut(&node) {
            n.state = NodeState::Failed;
            n.notes = "the reviewer ended without a verdict".into();
            n.ended_ms = apz_plaza::now_ms();
        }
    });
    tick(app);
}

// ── the gate ─────────────────────────────────────────────────────────────────────────────────────────────────────

/// The gate's decision for a run: the rules that fired and what they decide.
pub fn decide(run: &Run) -> Outcome {
    let lows = run.lows();
    let mut rework = Vec::new();
    let mut owner = Vec::new();
    let mut replan = Vec::new();
    let mut fail = Vec::new();
    for rule in &run.rules {
        let Ok(cond) = recipe::parse_condition(&rule.when) else {
            continue;
        };
        let fired = match &cond {
            Condition::AnyFailed => {
                let failed: Vec<&str> = run
                    .nodes
                    .iter()
                    .filter(|n| n.state == NodeState::Failed)
                    .map(|n| n.def.id.as_str())
                    .collect();
                (!failed.is_empty()).then(|| format!("{} did not pass", failed.join(", ")))
            }
            Condition::NodeFailed(id) => run
                .node(id)
                .filter(|n| n.state == NodeState::Failed)
                .map(|n| format!("{} did not pass: {}", id, n.notes)),
            Condition::Score {
                axis,
                at_most,
                measured,
            } => lows
                .get(axis)
                .filter(|(l, m)| *l <= *at_most && (!measured || *m))
                .map(|(l, _)| format!("{} is {l} (rule: {})", recipe::axis_name(axis), rule.when)),
        };
        if let Some(reason) = fired {
            match rule.then.as_str() {
                "rework" => rework.push(reason),
                "owner" => owner.push(reason),
                "replan" => replan.push(reason),
                "fail" => fail.push(reason),
                _ => {}
            }
        }
    }
    let now = apz_plaza::now_ms();
    let (decision, mut reasons) = if !fail.is_empty() {
        ("fail", fail)
    } else if !rework.is_empty() {
        if run.round > run.rounds {
            (
                "owner",
                rework
                    .into_iter()
                    .chain(std::iter::once(format!(
                        "{} rework rounds are used up",
                        run.rounds
                    )))
                    .collect(),
            )
        } else {
            ("rework", rework)
        }
    } else if !owner.is_empty() || run.owner_asked {
        (
            "owner",
            if owner.is_empty() {
                vec!["a first rule asks the owner".into()]
            } else {
                owner
            },
        )
    } else {
        ("pass", vec![])
    };
    reasons.dedup();
    Outcome {
        decision: decision.into(),
        reasons,
        replan,
        at_ms: now,
        by: "gate".into(),
    }
}

/// Closes a run's gate and acts on it.
fn gate(app: &App, run_id: &str) {
    let Some(run) = app.reviews.run(run_id) else {
        return;
    };
    if run.state != RunState::Running {
        return;
    }
    // One tick decides: the gate is taken (waiting → running) before anything is attested or sent.
    let Some(gate_id) = run
        .nodes
        .iter()
        .find(|n| n.def.kind == "gate")
        .map(|n| n.def.id.clone())
    else {
        return;
    };
    if !app.reviews.claim(run_id, &gate_id) {
        return;
    }
    let outcome = decide(&run);
    // A re-check or a post-land suite decides on its own (its revision may not be one Weft has).
    if run.purpose != "review" {
        return settle_check(app, &run, outcome);
    }
    let record = app
        .weft
        .hub("attest", json!({ "revision": run.revision, "digest": revision_digest(app, &run.revision), "predicate_type": "gate", "predicate": { "run": run.id, "recipe": run.recipe, "round": run.round, "outcome": outcome.decision, "reasons": outcome.reasons, "lows": run.lows() } }))
        .ok()
        .and_then(|v| v["seq"].as_u64());
    let updated = app.reviews.update(run_id, |r| {
        if let Some(g) = r.nodes.iter_mut().find(|n| n.def.kind == "gate") {
            g.state = if outcome.decision == "pass" {
                NodeState::Passed
            } else {
                NodeState::Failed
            };
            g.record = record;
            g.notes = outcome.reasons.join("; ");
            g.ended_ms = apz_plaza::now_ms();
        }
        r.state = match outcome.decision.as_str() {
            "pass" => RunState::Passed,
            "rework" => RunState::Rework,
            "owner" => RunState::Owner,
            _ => RunState::Failed,
        };
        r.outcome = Some(outcome.clone());
    });
    let Some(run) = updated else { return };
    if !outcome.replan.is_empty() {
        app.bus.publish("review_replan", &json!({ "run": run.id, "task": run.task, "project": run.project, "reasons": outcome.replan }));
    }
    app.bus.publish(
        "review_run",
        &json!({ "run": run.id, "task": run.task, "state": outcome.decision }),
    );
    match outcome.decision.as_str() {
        "pass" => passed(app, &run, "the review graph passed it"),
        "rework" => send_back(app, &run, &outcome.reasons),
        "owner" => ask_owner(app, &run),
        _ => failed(app, &run, &outcome.reasons.join("; ")),
    }
}

/// A pass: the revision joins its project's landing queue; the task is done when it lands.
fn passed(app: &App, run: &Run, _notes: &str) {
    let detail = app.weft.get(&format!("/changes/{}", run.change)).ok();
    if let Some(d) = detail {
        let n = d["rounds"].as_array().map(|r| r.len()).unwrap_or(0);
        if let Err(e) = app.weft.hub(
            "queue.add",
            json!({ "repo": d["repo"], "ref": d["ref"], "change": run.change, "n": n }),
        ) {
            eprintln!("apid: {} did not join its queue: {e}", run.change);
        }
    }
}

fn failed(app: &App, run: &Run, notes: &str) {
    let _ = app.work.owner_verdict(app, &run.task, false, notes);
}

/// Rework: the task goes back to its worker with the findings, the lane stays open; its next seal starts the next
/// round.
fn send_back(app: &App, run: &Run, reasons: &[String]) {
    let Some(task) = app.plaza.tasks.find(&run.task) else {
        return;
    };
    let findings: Vec<String> = run
        .nodes
        .iter()
        .flat_map(|n| n.findings.iter())
        .map(|f| format!("- {} ({}, {}): {}", f.symbol, f.node, f.severity, f.text))
        .collect();
    let note = format!(
        "Sent back by review {} (round {}): {}",
        run.id,
        run.round,
        reasons.join("; ")
    );
    if task.rework(Some(note.clone())).is_err() {
        return;
    }
    let lane = app.weft.lane_of(&run.task).map(|change| {
        let repo = app
            .weft
            .get(&format!("/changes/{change}"))
            .ok()
            .and_then(|d| d["repo"].as_str().map(str::to_string))
            .unwrap_or_default();
        apz_proto::wire::WorkLane { change, repo }
    });
    let prompt = format!(
        "{goal}\n\n{note}\n\nWhat the review found:\n{found}\n\nFix these in your lane{lane} and finish again; the next review round reads your next revision.",
        goal = task.goal(),
        found = if findings.is_empty() {
            "- (no findings beyond the reasons above)".into()
        } else {
            findings.join("\n")
        },
        lane = lane
            .as_ref()
            .map(|l| format!(" {} (./{}/)", l.change, l.repo))
            .unwrap_or_default()
    );
    let origin = Origin {
        task: Some(run.task.clone()),
        lane,
        ..Origin::default()
    };
    app.work.enqueue(
        app,
        run.worker,
        WorkKind::Task,
        format!("task-{}", run.task),
        prompt,
        origin,
    );
}

/// The owner's card: one approval of kind `review`, drawn as a review card.
fn ask_owner(app: &App, run: &Run) {
    let worker = app
        .agents
        .get(run.worker)
        .map(|a| a.label())
        .unwrap_or_else(|| format!("agent {}", run.worker));
    let goal = app
        .plaza
        .tasks
        .find(&run.task)
        .map(|t| t.goal().to_string())
        .unwrap_or_default();
    let detail = json!({
        "task": run.task, "run": run.id, "cid": run.worker, "worker": worker, "goal": goal, "summary": run.summary,
        "verdict": run.sentence(), "needs_you": run.outcome.as_ref().map(|o| o.reasons.join("; ")).unwrap_or_default(),
        "readings": run.readings().into_iter().map(|(n, v)| json!({ "name": n, "level": v })).collect::<Vec<_>>(),
        "change": run.change, "revision": run.revision, "round": run.round, "choices": ["pass", "rework", "fail"],
    });
    let item = app.approvals.submit(
        run.worker,
        "review",
        &format!("review of task {}", run.task),
        detail,
    );
    crate::approvals::announce(app, &item);
}

/// The owner's ruling on a review card: pass, send back, or fail. Signed for the owner from the console.
pub fn owner_ruled(
    app: &App,
    item: &crate::approvals::Approval,
    approve: bool,
    action: Option<&str>,
) -> Result<(), String> {
    let Some(run_id) = item.detail.get("run").and_then(Value::as_str) else {
        // A card from before the review graph (no run): the old path.
        if action == Some("rework") {
            return Err("this review card has no review run to send back".into());
        }
        return crate::scheduler::review_decided(app, item, approve);
    };
    let run = app.reviews.run(run_id).ok_or("no such review run")?;
    let decision = match (approve, action) {
        (_, Some("rework")) => "rework",
        (true, _) => "pass",
        (false, _) => "fail",
    };
    let notes = item
        .reason
        .clone()
        .unwrap_or_else(|| format!("the owner ruled {decision}"));
    let signed = app.weft.console("attest", json!({ "revision": run.revision, "digest": revision_digest(app, &run.revision), "predicate_type": "ruling", "predicate": { "run": run.id, "decision": decision, "notes": notes } }));
    if let Err(e) = signed {
        eprintln!("apid: the owner's ruling on {run_id} was not signed in Weft: {e}");
    }
    let updated = app.reviews.update(run_id, |r| {
        r.state = match decision {
            "pass" => RunState::Passed,
            "rework" => RunState::Rework,
            _ => RunState::Failed,
        };
        r.outcome = Some(Outcome {
            decision: decision.into(),
            reasons: vec![notes.clone()],
            replan: vec![],
            at_ms: apz_plaza::now_ms(),
            by: "owner".into(),
        });
    });
    let run = updated.ok_or("no such review run")?;
    match decision {
        "pass" => passed(app, &run, &notes),
        "rework" => send_back(app, &run, std::slice::from_ref(&notes)),
        _ => failed(app, &run, &notes),
    }
    app.bus.publish(
        "review_run",
        &json!({ "run": run.id, "task": run.task, "state": decision, "by": "owner" }),
    );
    Ok(())
}

/// Findings of earlier rounds on definitions still in this round's tree (they follow the definition's lineage).
pub fn carried_findings(app: &App, run: &Run) -> Vec<Finding> {
    let lineages: Vec<String> = run.tree["files"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|f| f["symbols"].as_array().cloned().unwrap_or_default())
        .filter_map(|s| s["lineage"].as_str().map(str::to_string))
        .collect();
    app.reviews
        .runs_of(&run.task)
        .into_iter()
        .filter(|r| r.round < run.round)
        .flat_map(|r| r.nodes.into_iter().flat_map(|n| n.findings))
        .filter(|f| f.lineage.as_ref().is_some_and(|l| lineages.contains(l)))
        .collect()
}

// ── re-checks and the post-land suite ────────────────────────────────────────────────────────────────────────────

/// A one-step run: a check node with `commands`, then the gate (a failed check fails it).
fn check_run(
    app: &App,
    purpose: &str,
    task: &str,
    project: &str,
    worker: u64,
    change: &str,
    revision: &str,
    repo: &str,
    commands: Vec<String>,
) -> Option<String> {
    if commands.is_empty() {
        return None;
    }
    let check = NodeDef {
        id: "checks".into(),
        kind: "check".into(),
        role: Some("qa".into()),
        axes: vec!["tst".into()],
        commands,
        after: vec![],
        ..Default::default()
    };
    let gate = NodeDef {
        id: "gate".into(),
        kind: "gate".into(),
        after: vec!["checks".into()],
        ..Default::default()
    };
    let node = |def: NodeDef| NodeRun {
        def,
        parent: None,
        state: NodeState::Waiting,
        assignee: None,
        work: None,
        tries: 0,
        scores: vec![],
        findings: vec![],
        touches: vec![],
        notes: String::new(),
        record: None,
        started_ms: 0,
        ended_ms: 0,
        spawned: 0,
    };
    let now = apz_plaza::now_ms();
    let id = app.reviews.with(|b| {
        b.next += 1;
        let id = format!("rv{}", b.next);
        b.runs.insert(
            id.clone(),
            Run {
                id: id.clone(),
                task: task.into(),
                project: project.into(),
                worker,
                change: change.into(),
                revision: revision.into(),
                recipe: format!("{purpose}@1"),
                round: 1,
                rounds: 0,
                kind: "code".into(),
                state: RunState::Running,
                nodes: vec![node(check), node(gate)],
                rules: vec![recipe::Rule {
                    when: "failed".into(),
                    then: "fail".into(),
                }],
                owner_asked: false,
                outcome: None,
                tree: Value::Null,
                summary: String::new(),
                purpose: purpose.into(),
                repo: repo.into(),
                created_ms: now,
                updated_ms: now,
            },
        );
        id
    });
    app.bus.publish(
        "review_run",
        &json!({ "run": id, "task": task, "state": "running", "purpose": purpose }),
    );
    Some(id)
}

/// The commands a task's code recipe re-checks with (its `checks` node), and its post-land suite.
fn recipe_commands(app: &App, task: &str) -> (Vec<String>, Vec<String>) {
    let Some(run) = app
        .reviews
        .runs_of(task)
        .into_iter()
        .find(|r| r.purpose == "review")
    else {
        return (vec![], vec![]);
    };
    let (id, version) = run
        .recipe
        .split_once('@')
        .map(|(i, v)| (i.to_string(), v.parse().ok()))
        .unwrap_or((run.recipe.clone(), None));
    let Some(recipe) = app.reviews.recipe(&id, version) else {
        return (vec![], vec![]);
    };
    let checks = recipe
        .nodes
        .iter()
        .filter(|n| n.kind == "check")
        .flat_map(|n| n.commands.clone())
        .collect();
    (checks, recipe.post_land.clone())
}

/// A merged landing candidate waits for its checks: a re-check run for it.
pub fn recheck(app: &App, repo: &str, change: &str, digest: &str) -> Option<String> {
    if app
        .reviews
        .book
        .lock()
        .ok()?
        .runs
        .values()
        .any(|r| r.purpose == "recheck" && r.revision == digest)
    {
        return None;
    }
    let detail = app.weft.get(&format!("/changes/{change}")).ok()?;
    let task = detail["task"].as_str()?.to_string();
    let project = app.plaza.tasks.find(&task)?.project()?.to_string();
    let worker = detail["author"]
        .as_str()?
        .strip_prefix("agent:")?
        .parse()
        .ok()?;
    let (checks, _) = recipe_commands(app, &task);
    match check_run(
        app, "recheck", &task, &project, worker, change, digest, repo, checks,
    ) {
        Some(id) => Some(id),
        None => {
            // Nothing to re-check with: the candidate lands as it is.
            let _ = app.weft.post(
                &format!("/repos/{repo}/queue/report"),
                &json!({ "digest": digest, "ok": true }),
            );
            None
        }
    }
}

/// After a landing: the recipe's slow checks on main, as a post-land run.
pub fn post_land(app: &App, repo: &str, change: &str, task: &str) {
    let Some(record) = app.plaza.tasks.find(task) else {
        return;
    };
    let Some(project) = record.project().map(str::to_string) else {
        return;
    };
    let worker = record.view().agent_cid.unwrap_or(0);
    let (_, suite) = recipe_commands(app, task);
    check_run(
        app,
        "post-land",
        task,
        &project,
        worker,
        change,
        &format!("{change}.l"),
        repo,
        suite,
    );
}

/// A re-check's or post-land suite's outcome.
fn settle_check(app: &App, run: &Run, outcome: Outcome) {
    let pass = outcome.decision == "pass";
    let notes = run
        .nodes
        .iter()
        .find(|n| n.def.id == "checks")
        .map(|n| n.notes.clone())
        .unwrap_or_default();
    app.reviews.update(&run.id, |r| {
        if let Some(g) = r.nodes.iter_mut().find(|n| n.def.kind == "gate") {
            g.state = if pass {
                NodeState::Passed
            } else {
                NodeState::Failed
            };
            g.ended_ms = apz_plaza::now_ms();
        }
        r.state = if pass {
            RunState::Passed
        } else {
            RunState::Failed
        };
        r.outcome = Some(outcome.clone());
    });
    app.bus.publish("review_run", &json!({ "run": run.id, "task": run.task, "state": outcome.decision, "purpose": run.purpose }));
    match (run.purpose.as_str(), pass) {
        ("recheck", ok) => {
            let _ = app.weft.post(
                &format!("/repos/{}/queue/report", run.repo),
                &json!({ "digest": run.revision, "ok": ok }),
            );
            if !ok {
                // A merge that breaks the checks goes back to the worker; the change left the queue.
                let reasons = vec![format!(
                    "the re-check after merging onto main failed: {notes}"
                )];
                send_back(app, run, &reasons);
            }
        }
        ("post-land", false) => {
            // Main is broken: a signed revert lands at once, and the author gets a fix task.
            let reason = format!("the post-land suite failed on {}: {notes}", run.revision);
            match app.weft.post(
                &format!("/changes/{}/revert", run.change),
                &json!({ "reason": reason }),
            ) {
                Ok(v) => app.bus.publish(
                    "weft_revert",
                    &json!({ "change": run.change, "revert": v["change"], "reason": reason }),
                ),
                Err(e) => eprintln!("apid: reverting {}: {e}", run.change),
            }
            let goal = format!("Fix what {} broke: {reason}", run.change);
            let created = app.plaza.tasks.create(&apz_plaza::tasks::CreateTask {
                goal,
                acceptance: Some("the post-land suite passes on main".into()),
                role: app.agents.get(run.worker).map(|a| a.role),
                after: vec![],
                project: Some(run.project.clone()),
            });
            if let Ok(t) = created {
                let _ = app.projects.link_task(&run.project, t.id());
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn run_with(state: RunState) -> Run {
        let node = |id: &str, kind: &str| NodeRun {
            def: NodeDef {
                id: id.into(),
                kind: kind.into(),
                ..Default::default()
            },
            parent: None,
            state: NodeState::Waiting,
            assignee: None,
            work: None,
            tries: 0,
            scores: vec![],
            findings: vec![],
            touches: vec![],
            notes: String::new(),
            record: None,
            started_ms: 0,
            ended_ms: 0,
            spawned: 0,
        };
        Run {
            id: "rv1".into(),
            task: "t1".into(),
            project: "p1".into(),
            worker: 300,
            change: "w-1".into(),
            revision: "w-1.r1".into(),
            recipe: "code@3".into(),
            round: 1,
            rounds: 1,
            kind: "code".into(),
            state,
            nodes: vec![node("quality", "agent"), node("gate", "gate")],
            rules: vec![],
            owner_asked: false,
            outcome: None,
            tree: Value::Null,
            summary: String::new(),
            purpose: "review".into(),
            repo: String::new(),
            created_ms: 0,
            updated_ms: 0,
        }
    }

    #[test]
    fn of_ticks_that_start_one_node_at_once_only_one_takes_it() {
        let reviews = Arc::new(Reviews::default());
        reviews.with(|b| b.runs.insert("rv1".into(), run_with(RunState::Running)));
        let taken: usize = (0..8)
            .map(|_| {
                let reviews = Arc::clone(&reviews);
                std::thread::spawn(move || reviews.claim("rv1", "quality"))
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| usize::from(t.join().unwrap()))
            .sum();
        assert_eq!(taken, 1);
        let run = reviews.run("rv1").unwrap();
        assert_eq!(run.node("quality").unwrap().state, NodeState::Running);
        // The gate is taken the same way, once.
        assert!(reviews.claim("rv1", "gate"));
        assert!(!reviews.claim("rv1", "gate"));
    }

    #[test]
    fn a_node_of_a_run_that_ended_is_not_taken() {
        let reviews = Reviews::default();
        reviews.with(|b| b.runs.insert("rv1".into(), run_with(RunState::Rework)));
        assert!(!reviews.claim("rv1", "quality"));
        assert!(!reviews.claim("rv2", "quality"));
        assert_eq!(
            reviews.run("rv1").unwrap().node("quality").unwrap().state,
            NodeState::Waiting
        );
    }
}
