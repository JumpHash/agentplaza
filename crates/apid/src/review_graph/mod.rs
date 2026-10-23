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
//!
//! Kept in plaza.db (table `review_graph`; its runs, recipes and proposals are child tables). A run's change tree is
//! in the object store (apz-blobs), under the hash the run keeps.

pub mod recipe;
pub mod routes;
pub mod tools;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use apz_blobs::{Blobs, Hash};
use apz_store::rusqlite::Connection;
use apz_store::{RowSet, StoreError, TableDef};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::scheduler::Origin;
use crate::state::App;
use apz_proto::wire::WorkKind;
use recipe::{Condition, NodeDef, Recipe};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Running,
    Passed,
    Rework,
    Owner,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
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
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, apz_store::Table)]
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

/// A finding's verdict (8.3): pending until the verifier rules on it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, apz_store::Column)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    #[default]
    Pending,
    Confirmed,
    Dropped,
}

/// Who acted: the seat, the model and the session (8.3's `by` and `verified_by`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, apz_store::Table)]
pub struct By {
    pub seat: String,
    pub model: String,
    #[serde(default)]
    pub session: String,
}

/// Which model filled a node in a run: the author's (`author`) and each reviewer's and verifier's (8.3's `models`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, apz_store::Table)]
pub struct ModelUse {
    pub node: String,
    pub cid: u64,
    pub model: String,
    #[serde(default)]
    pub session: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, apz_store::Table)]
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
    /// `f<n>`, unique in its run: what the verifier and the author name it by.
    #[serde(default)]
    pub id: String,
    /// The failure: the inputs or state, and the wrong outcome they cause. A finding is refused without one.
    #[serde(default)]
    pub scenario: String,
    /// The kind of defect (the reviewer's axis unless it says): pitfalls are kinds confirmed twice.
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub verdict: Verdict,
    /// For a confirmed finding: the test, the run or the lines that show it.
    #[serde(default)]
    pub proof: String,
    /// For a dropped finding: why it is not a defect (or the planner's reason for accepting it).
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    #[table(child)]
    pub by: Option<By>,
    #[serde(default)]
    #[table(child)]
    pub verified_by: Option<By>,
    /// The author's contest of a confirmed finding (its reason); empty when not contested.
    #[serde(default)]
    pub contest: String,
    /// The planner's ruling on a contested finding: `fix`, `accept` or `drop`.
    #[serde(default)]
    pub ruling: String,
    #[serde(default)]
    pub ruling_reason: String,
}

fn minor() -> String {
    "minor".into()
}

/// What a part touches, in a few words (built from Weft's references, worded by the node).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, apz_store::Table)]
pub struct Touch {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub how: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, apz_store::Table)]
pub struct NodeRun {
    #[table(child)]
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
    #[table(child)]
    pub scores: Vec<Score>,
    #[serde(default)]
    #[table(child)]
    pub findings: Vec<Finding>,
    #[serde(default)]
    #[table(child)]
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

#[derive(Clone, Debug, Serialize, Deserialize, apz_store::Table)]
pub struct Outcome {
    /// `pass`, `rework`, `owner`, `fail`.
    pub decision: String,
    #[table(list)]
    pub reasons: Vec<String>,
    #[serde(default)]
    #[table(list)]
    pub replan: Vec<String>,
    pub at_ms: u64,
    #[serde(default)]
    pub by: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, apz_store::Table)]
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
    #[table(child)]
    pub nodes: Vec<NodeRun>,
    #[serde(default)]
    #[table(child = "review_graph_run_rule")]
    pub rules: Vec<recipe::Rule>,
    #[serde(default)]
    pub owner_asked: bool,
    #[serde(default)]
    #[table(child)]
    pub outcome: Option<Outcome>,
    /// The hash of the change tree the nodes read (from Weft, at the start) in the object store; read it with
    /// [`Reviews::tree`].
    #[serde(default)]
    pub tree_blob: Option<String>,
    #[serde(default)]
    pub summary: String,
    /// `review` (a sealed round), `recheck` (a landing candidate, `revision` is its digest) or `post-land` (main
    /// after the change landed).
    #[serde(default = "review_purpose")]
    pub purpose: String,
    /// For a re-check: the repo whose queue waits on it.
    #[serde(default)]
    pub repo: String,
    /// Which model filled each seat of the run (the author's, then each reviewer's and verifier's), for the gate and
    /// for evals that score reviewers.
    #[serde(default)]
    #[table(child = "review_graph_run_model")]
    pub models: Vec<ModelUse>,
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

impl Run {
    /// Every finding of the run, with the node that made it.
    pub fn findings(&self) -> impl Iterator<Item = &Finding> {
        self.nodes.iter().flat_map(|n| n.findings.iter())
    }

    fn finding_mut(&mut self, id: &str) -> Option<&mut Finding> {
        self.nodes
            .iter_mut()
            .flat_map(|n| n.findings.iter_mut())
            .find(|f| f.id == id)
    }

    /// A contested finding the verifier holds, which the planner has yet to rule on: the gate waits for the ruling.
    pub fn awaiting_ruling(&self) -> bool {
        self.findings().any(|f| {
            f.verdict == Verdict::Confirmed && !f.contest.is_empty() && f.ruling.is_empty()
        })
    }
}

/// The reviewers' and the verifier's model differs from the author's (8.2): the gap, named, or `None` when they all
/// differ. Fails closed: a run without its author's model has no gap it can show, so it is not admitted. The author
/// is recorded as `author`; every other entry is a reviewer or the verifier.
pub fn cross_model_gap(run: &Run) -> Option<String> {
    let Some(author) = run.models.iter().find(|m| m.node == "author") else {
        return Some("the author's model is not recorded".into());
    };
    if author.model.is_empty() {
        return Some("the author's model is not known".into());
    }
    run.models
        .iter()
        .filter(|m| m.node != "author")
        .find(|m| m.model == author.model)
        .map(|m| format!("{} ran on {}, the author's own model", m.node, m.model))
}

/// Records what a reviewer's step reports, with its findings (8.2, 8.3): a finding names its scenario, and gets its
/// `f<n>` id, its `by` and a pending verdict. Refused before anything is recorded when one has no scenario.
pub fn check_findings(findings: &[Finding]) -> Result<(), String> {
    for f in findings {
        if f.scenario.trim().is_empty() {
            return Err(format!(
                "{}: a finding names its scenario (the inputs or state, and the wrong outcome they cause)",
                f.symbol
            ));
        }
    }
    Ok(())
}

/// The verifier's ruling on finding `id` (8.2): confirmed with a proof (a test, a run, or the lines), or dropped with
/// its reason. A finding the author contested is ruled on again: a verifier that holds it leaves it to the planner.
pub fn rule_finding(
    run: &mut Run,
    id: &str,
    verdict: Verdict,
    proof: &str,
    reason: &str,
    by: By,
) -> Result<(), String> {
    let round = run.round;
    let f = run
        .finding_mut(id)
        .ok_or_else(|| format!("no finding {id} in this review"))?;
    if f.verdict != Verdict::Pending {
        return Err(format!("{id} is already ruled on"));
    }
    match verdict {
        Verdict::Confirmed if proof.trim().is_empty() => Err(
            "a confirmed finding names its proof: a failing test, a run, or the exact lines and the reasoning".into(),
        ),
        Verdict::Dropped if reason.trim().is_empty() => {
            Err("a dropped finding names its reason".into())
        }
        Verdict::Pending => Err("a ruling is confirmed or dropped".into()),
        Verdict::Confirmed => {
            f.verdict = Verdict::Confirmed;
            f.round = round;
            f.proof = proof.trim().into();
            f.verified_by = Some(by);
            Ok(())
        }
        Verdict::Dropped => {
            f.verdict = Verdict::Dropped;
            f.reason = reason.trim().into();
            f.verified_by = Some(by);
            Ok(())
        }
    }
}

/// The author contests a confirmed finding, once (8.5): the finding goes back to the verifier, and the run reopens
/// for the verifier's second look and the gate's next decision.
pub fn contest_finding(run: &mut Run, id: &str, reason: &str) -> Result<(), String> {
    if reason.trim().is_empty() {
        return Err("a contest names its reason".into());
    }
    let f = run
        .finding_mut(id)
        .ok_or_else(|| format!("no finding {id} in this review"))?;
    if f.verdict != Verdict::Confirmed {
        return Err(format!(
            "{id} is not confirmed, so there is nothing to contest"
        ));
    }
    if !f.contest.is_empty() {
        return Err(format!("{id} was contested already: a contest is once"));
    }
    f.contest = reason.trim().into();
    f.verdict = Verdict::Pending;
    Ok(())
}

/// Reopens a review-graph run for the verifier's second look at a contested finding: the run runs again and its
/// verifier and gate wait.
pub fn reopen_review(run: &mut Run) {
    run.state = RunState::Running;
    for n in run.nodes.iter_mut() {
        if n.def.role.as_deref() == Some("verifier") || n.def.kind == "gate" {
            n.state = NodeState::Waiting;
            n.ended_ms = 0;
        }
    }
}

/// The planner's ruling on a contested finding the verifier held (8.5): `fix` keeps it confirmed for the author,
/// `accept` and `drop` drop it with the planner's reason.
pub fn arbitrate_finding(
    run: &mut Run,
    id: &str,
    ruling: &str,
    reason: &str,
) -> Result<(), String> {
    let f = run
        .finding_mut(id)
        .ok_or_else(|| format!("no finding {id} in this review"))?;
    if !(f.verdict == Verdict::Confirmed && !f.contest.is_empty() && f.ruling.is_empty()) {
        return Err(format!("{id} is not waiting for a ruling"));
    }
    if reason.trim().is_empty() {
        return Err("a ruling names its reason".into());
    }
    match ruling {
        "fix" => {}
        "accept" | "drop" => {
            f.verdict = Verdict::Dropped;
            f.reason = reason.trim().into();
        }
        other => return Err(format!("{other} is not a ruling: fix, accept or drop")),
    }
    f.ruling = ruling.into();
    f.ruling_reason = reason.trim().into();
    Ok(())
}

/// The review graph's book: one row of `review_graph`, its runs, recipes and proposals as child rows.
#[derive(Default, apz_store::Table)]
#[table(name = "review_graph")]
struct Book {
    #[table(map = "review_graph_run")]
    runs: BTreeMap<String, Run>,
    next: u64,
    /// The owner's recipes and versions (the built-in ones are always there).
    #[table(child = "review_graph_recipe")]
    recipes: Vec<Recipe>,
    /// Proposals from the orchestrator, waiting for the owner.
    #[table(child = "review_graph_proposal")]
    proposals: Vec<Proposal>,
}

/// The review graph's table definition: `review_graph`, with its children.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(Book)
}

#[derive(Clone, Debug, Serialize, Deserialize, apz_store::Table)]
pub struct Proposal {
    pub id: String,
    pub by: u64,
    #[table(child)]
    pub recipe: Recipe,
    pub why: String,
    pub state: String,
    pub at_ms: u64,
}

/// The review graph's runs and recipes, kept in plaza.db (component `review_graph`). A run's change tree is in the
/// object store, under the hash the run keeps.
pub struct Reviews {
    book: Mutex<Book>,
    blobs: Arc<Blobs>,
}

impl Reviews {
    /// An empty review graph over the object store `blobs`; [`Reviews::load`] fills it at start.
    pub fn new(blobs: Arc<Blobs>) -> Reviews {
        Reviews {
            book: Mutex::new(Book::default()),
            blobs,
        }
    }

    /// Replaces the book with what plaza.db holds (at start).
    pub fn load(&self, conn: &Connection) -> Result<(), StoreError> {
        let stored: Option<Book> = apz_store::table::load_one(conn, def())?;
        *self.book.lock().unwrap_or_else(|p| p.into_inner()) = stored.unwrap_or_default();
        Ok(())
    }

    /// The book as rows of its tables, as it is now (the saver writes what changed).
    pub fn rows(&self) -> Result<RowSet, StoreError> {
        let book = self.book.lock().unwrap_or_else(|p| p.into_inner());
        RowSet::one(def(), &*book)
    }

    /// Puts a change tree in the object store and returns its hash in hex.
    fn put_tree(&self, tree: &Value) -> Result<String, String> {
        let bytes = serde_json::to_vec(tree).map_err(|e| e.to_string())?;
        self.blobs.put(&bytes).map(|hash| hash.hex())
    }

    /// The change tree a run started with, read back from the object store; `Value::Null` when it has none or it
    /// cannot be read.
    pub fn tree(&self, run: &Run) -> Value {
        let Some(hash) = run
            .tree_blob
            .as_deref()
            .and_then(|h| h.parse::<Hash>().ok())
        else {
            return Value::Null;
        };
        let bytes = match self.blobs.get(&hash) {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!(
                    "apid: the change tree {} of run {} cannot be read: {e}",
                    hash.short(),
                    run.id
                );
                return Value::Null;
            }
        };
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            eprintln!(
                "apid: the change tree {} of run {} is not JSON: {e}",
                hash.short(),
                run.id
            );
            Value::Null
        })
    }

    fn with<T>(&self, f: impl FnOnce(&mut Book) -> T) -> T {
        let mut book = self.book.lock().unwrap_or_else(|p| p.into_inner());
        f(&mut book)
    }

    /// Edits run `id` under the book's lock: what `f` refuses leaves the run as it was.
    fn edit<T>(
        &self,
        id: &str,
        f: impl FnOnce(&mut Run) -> Result<T, String>,
    ) -> Result<T, String> {
        self.with(|b| {
            let r = b.runs.get_mut(id).ok_or("no such review")?;
            let out = f(r)?;
            r.updated_ms = apz_plaza::now_ms();
            Ok(out)
        })
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

    /// How many confirmed findings of kind `kind` the project's runs hold (a pitfall is a kind confirmed twice).
    pub fn confirmed_of_kind(&self, project: &str, kind: &str) -> usize {
        let book = self.book.lock().unwrap_or_else(|p| p.into_inner());
        book.runs
            .values()
            .filter(|r| r.project == project)
            .flat_map(|r| r.findings())
            .filter(|f| f.verdict == Verdict::Confirmed && f.kind == kind)
            .count()
    }

    /// The task's latest run, when it is in rework: the only one an author may contest a finding of.
    pub fn reworked_run_of(&self, task: &str) -> Option<Run> {
        self.runs_of(task)
            .into_iter()
            .next()
            .filter(|r| r.state == RunState::Rework)
    }

    /// The task's review run on the work loop's seats (purpose `seat`): one run for the task, reopened per round.
    pub fn seat_run(&self, task: &str) -> Option<Run> {
        self.runs_of(task).into_iter().find(|r| r.purpose == "seat")
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
    let tree_blob = match app.reviews.put_tree(&tree) {
        Ok(hash) => hash,
        Err(e) => {
            eprintln!("apid: the change tree of {revision} was not stored: {e}");
            return None;
        }
    };
    let project_kind = app
        .projects
        .get(&project)
        .map(|p| p.kind)
        .unwrap_or_default();
    let kind = recipe::kind_of(&tree, &project_kind).to_string();
    let recipe = pick_recipe(app, &kind, record.role().unwrap_or(""))?;
    let (nodes, owner) = plan(&recipe, &tree);
    let author = model_use(app, "author", worker);
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
                tree_blob: Some(tree_blob),
                summary: view.note.unwrap_or_default(),
                purpose: review_purpose(),
                repo: String::new(),
                models: vec![author.clone()],
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
    // The run is also a run of the engine's `review` recipe (recipes/runs.rs), which records its outcome.
    let project = app.reviews.run(&id).map(|r| r.project).unwrap_or_default();
    app.recipe_runs.start_engine(
        "review",
        "review",
        Some(task),
        Some(&project),
        Some(&id),
        apz_plaza::now_ms(),
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
    // The verifier proves what the reviewers found, so it waits for every other node (the added reviewers too).
    let others: Vec<String> = nodes
        .iter()
        .filter(|n| n.def.kind != "gate" && n.def.role.as_deref() != Some("verifier"))
        .map(|n| n.def.id.clone())
        .collect();
    for n in nodes
        .iter_mut()
        .filter(|n| n.def.role.as_deref() == Some("verifier"))
    {
        n.def.after = others.clone();
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
                .filter(|r| r.state == RunState::Running && r.purpose != "seat")
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
            && !run.awaiting_ruling()
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
            let tree = app.reviews.tree(run);
            let (scores, notes) = match n.def.tool.as_deref() {
                Some("divergence") => divergence_scores(&tree),
                _ => (
                    vec![],
                    format!(
                        "{} files, +{} −{}",
                        tree["files"].as_array().map(|f| f.len()).unwrap_or(0),
                        tree["add"],
                        tree["del"]
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
            if n.def.role.as_deref() == Some("verifier")
                && !run.findings().any(|f| f.verdict == Verdict::Pending)
            {
                // Nothing to prove: the verifier has no session to start.
                app.reviews.update(&run.id, |r| {
                    if let Some(n) = r.node_mut(node) {
                        n.state = NodeState::Passed;
                        n.notes = "no finding to prove".into();
                        n.ended_ms = apz_plaza::now_ms();
                    }
                });
                return;
            }
            let role = n
                .def
                .role
                .clone()
                .unwrap_or_else(|| crate::scheduler::REVIEWER_ROLE.into());
            let taken: Vec<u64> = run.nodes.iter().filter_map(|x| x.assignee).collect();
            let avoid = avoid_models(run, &n.def);
            let Some(cid) = pick_agent(app, &run.project, &role, run.worker, &taken, &avoid) else {
                // No agent of that role: a reviewer takes it, else it waits for the next tick (the timeout raises the
                // owner's card).
                if role != crate::scheduler::REVIEWER_ROLE
                    && let Some(cid) = pick_agent(
                        app,
                        &run.project,
                        crate::scheduler::REVIEWER_ROLE,
                        run.worker,
                        &taken,
                        &avoid,
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

/// The models a node's agent should not run on: the author's, and for the verifier the reviewers' too (8.2: the
/// verifier is a fourth model where one exists). A preference: the gate at the queue is what refuses a same-model review.
fn avoid_models(run: &Run, def: &NodeDef) -> Vec<String> {
    let mut out: Vec<String> = run
        .models
        .iter()
        .filter(|m| m.node == "author" && !m.model.is_empty())
        .map(|m| m.model.clone())
        .collect();
    if def.role.as_deref() == Some("verifier") {
        out.extend(
            run.models
                .iter()
                .filter(|m| m.node != "author")
                .map(|m| m.model.clone()),
        );
    }
    out
}

/// The model and session of the agent `cid`, as a run records the seat it fills.
fn model_use(app: &App, node: &str, cid: u64) -> ModelUse {
    ModelUse {
        node: node.into(),
        cid,
        model: app
            .agents
            .get(cid)
            .map(|a| crate::agents::model_of(app, &a))
            .unwrap_or_default(),
        session: app.sessions.by_cid(cid).map(|s| s.id).unwrap_or_default(),
    }
}

/// An agent of `role` working on `project`, not the worker, preferring one not already on this run, one whose model is
/// not in `avoid`, and one idle.
fn pick_agent(
    app: &App,
    project: &str,
    role: &str,
    worker: u64,
    taken: &[u64],
    avoid: &[String],
) -> Option<u64> {
    let on_project = crate::projects::agents_of(app, project);
    let found = app.agents.select(|a| {
        (a.role == role
            && a.cid != worker
            && on_project.contains(&a.cid)
            && a.status != crate::agents::Status::Offline
            && a.kind != "model")
            .then(|| (a.cid, a.status == crate::agents::Status::Idle))
    });
    let mut candidates: Vec<(u64, bool, usize)> = found
        .into_iter()
        .map(|(cid, idle)| {
            let model = app
                .agents
                .get(cid)
                .map(|a| crate::agents::model_of(app, &a))
                .unwrap_or_default();
            (cid, idle, usize::from(avoid.contains(&model)))
        })
        .collect();
    candidates.sort_by_key(|(cid, idle, same)| (taken.contains(cid), *same, !idle, *cid));
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
    let used = model_use(app, node, cid);
    app.reviews.update(&run.id, |r| {
        if let Some(n) = r.node_mut(node) {
            n.work = Some(work);
        }
        // Agent nodes (reviewers and the verifier) are what the cross-model gate compares with the author's model.
        if n.def.kind == "agent" {
            r.models.retain(|m| m.node != node);
            r.models.push(used);
        }
    });
    app.bus.publish(
        "review_node",
        &json!({ "run": run.id, "node": node, "state": "running", "cid": cid }),
    );
}

/// A reviewer's standing instructions: the change is for breaking, every finding names its scenario, and the project's
/// pitfalls (8.2, 8.6). The review graph's nodes and the work loop's review seats both read it.
pub fn sceptic_text(app: &App, project: &str) -> String {
    let mut p = String::from(
        "Your posture is a sceptic's: look for how this change breaks, not for reasons to pass it. Every finding names its scenario: the inputs or the state, and the wrong outcome they cause. A finding without one is refused.\n",
    );
    let pitfalls: Vec<String> = crate::project_memory::entries(app, project)
        .into_iter()
        .filter(|e| e.kind == "pitfall" && (e.status == "active" || e.status == "flagged"))
        .take(20)
        .map(|e| format!("- {}: {}", e.title, e.body))
        .collect();
    if !pitfalls.is_empty() {
        p.push_str(&format!(
            "This project's pitfalls (confirmed before; check the change against them):\n{}\n",
            pitfalls.join("\n")
        ));
    }
    p
}

/// A verifier's instructions, and the findings it rules on now (pending), with the contests the author made.
pub fn verifier_text(run: &Run) -> String {
    let mut p = String::from(
        "You verify; you do not review. For each finding below, prove it or drop it with finding_verify: confirmed (with the proof: a failing test you wrote, a reproduction you ran, or the exact lines and the reasoning you checked) or dropped (with the reason it is not a defect). Read the change with review_tree first. Finish with your verdict, pass, once every finding is ruled on.\n",
    );
    for f in run.findings().filter(|f| f.verdict == Verdict::Pending) {
        p.push_str(&format!(
            "- {} {} ({}, {}): {}\n  Scenario: {}\n",
            f.id, f.symbol, f.node, f.severity, f.text, f.scenario
        ));
    }
    for f in run
        .findings()
        .filter(|f| !f.contest.is_empty() && f.verdict == Verdict::Pending)
    {
        p.push_str(&format!(
            "The author contests {}: {}. Look again at it: the verdict stands or falls on your proof.\n",
            f.id, f.contest
        ));
    }
    p
}

/// The findings of a run's current round as its author reads them: the confirmed ones with their scenario and proof,
/// and the dropped ones with their reason (nothing to change for those).
fn findings_text(run: &Run) -> (Vec<String>, Vec<String>) {
    let confirmed = run
        .findings()
        .filter(|f| f.verdict == Verdict::Confirmed && f.round == run.round)
        .map(|f| {
            format!(
                "- {} {} ({}, {}): {} Scenario: {} Proof: {}",
                f.id, f.symbol, f.node, f.severity, f.text, f.scenario, f.proof
            )
        })
        .collect();
    let dropped = run
        .findings()
        .filter(|f| f.verdict == Verdict::Dropped && f.round == run.round)
        .map(|f| format!("- {} {}: not a defect: {}", f.id, f.symbol, f.reason))
        .collect();
    (confirmed, dropped)
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
    } else if n.def.role.as_deref() == Some("verifier") {
        p.push_str(&verifier_text(run));
    } else {
        p.push_str(&sceptic_text(app, &run.project));
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
    let (axes, verifier) = r
        .node(&node)
        .map(|n| {
            (
                n.def.axes.clone(),
                n.def.role.as_deref() == Some("verifier"),
            )
        })
        .unwrap_or_default();
    if verifier && !findings.is_empty() {
        return Err("a verifier rules on findings (finding_verify); it records none".into());
    }
    check_findings(&findings)?;
    let used = model_use(app, &node, cid);
    let by = By {
        seat: node.clone(),
        model: used.model,
        session: used.session,
    };
    let default_kind = axes.first().cloned().unwrap_or_else(|| "review".into());
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
    let tree = app.reviews.tree(&r);
    let lineage = |sym: &str| lineage_of(&tree, sym);
    let n_scores = scores.len();
    app.reviews.update(&run, |r| {
        let round = r.round;
        // `f<n>` numbers the run's findings, so the verifier and the author can name each one.
        let base: usize = r.nodes.iter().map(|n| n.findings.len()).sum();
        if let Some(n) = r.node_mut(&node) {
            for mut s in scores {
                s.node = node.clone();
                s.lineage = s.lineage.or_else(|| lineage(&s.symbol));
                n.scores
                    .retain(|x| !(x.symbol == s.symbol && x.axis == s.axis));
                n.scores.push(s);
            }
            for (i, mut f) in findings.into_iter().enumerate() {
                f.node = node.clone();
                f.round = round;
                f.id = format!("f{}", base + i + 1);
                f.verdict = Verdict::Pending;
                f.by = Some(by.clone());
                if f.kind.is_empty() {
                    f.kind = default_kind.clone();
                }
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
    if pass && n.def.role.as_deref() == Some("verifier") {
        let open: Vec<&str> = r
            .findings()
            .filter(|f| f.verdict == Verdict::Pending)
            .map(|f| f.id.as_str())
            .collect();
        if !open.is_empty() {
            return Err(format!(
                "rule on every finding first (finding_verify): {}",
                open.join(", ")
            ));
        }
    }
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

/// The verifier's ruling on a finding (`finding_verify`, 8.2): confirmed with its proof, or dropped with its reason. A
/// confirmed finding that the author contested goes to the planner (8.5); a kind confirmed for the second time in the
/// project proposes a pitfall (8.6).
pub fn verify(
    app: &App,
    cid: u64,
    work: u64,
    finding: &str,
    verdict: Verdict,
    proof: &str,
    reason: &str,
) -> Result<String, String> {
    let (run_id, node) = app
        .reviews
        .step_for(cid, work)
        .ok_or("you have no review step to rule in")?;
    let run = app.reviews.run(&run_id).ok_or("no such review")?;
    let is_verifier = run
        .node(&node)
        .is_some_and(|n| n.def.role.as_deref() == Some("verifier"));
    if !is_verifier {
        return Err("only the verifier of a review rules on its findings".into());
    }
    let used = model_use(app, &node, cid);
    let by = By {
        seat: node.clone(),
        model: used.model,
        session: used.session,
    };
    app.reviews.edit(&run_id, |r| {
        rule_finding(r, finding, verdict, proof, reason, by.clone())
    })?;
    let run = app.reviews.run(&run_id).ok_or("no such review")?;
    let f = run
        .findings()
        .find(|f| f.id == finding)
        .cloned()
        .ok_or("no such finding")?;
    if verdict == Verdict::Confirmed {
        if f.contest.is_empty() {
            // The second confirmation of one kind in the project is a pitfall the curator may land (8.6).
            if app.reviews.confirmed_of_kind(&run.project, &f.kind) == 2 {
                propose_pitfall(app, &run, &f, cid);
            }
        } else {
            request_ruling(app, &run, &f);
        }
    }
    tick(app);
    Ok(format!(
        "{finding}: {}",
        match verdict {
            Verdict::Confirmed => "confirmed",
            _ => "dropped",
        }
    ))
}

/// A pitfall proposal (2.2's `memory_propose`, kind `pitfall`) anchored to the finding's definition.
fn propose_pitfall(app: &App, run: &Run, f: &Finding, cid: u64) {
    let proposal = crate::project_memory::Proposal {
        id: String::new(),
        project: run.project.clone(),
        kind: "pitfall".into(),
        text: clip(
            &format!(
                "Findings of kind {} are confirmed here: {}",
                f.kind, f.scenario
            ),
            1000,
        ),
        anchors: vec![f.symbol.clone()],
        why: clip(
            &format!(
                "{} confirmed twice in this project (review {}, finding {}): {}",
                f.kind, run.id, f.id, f.text
            ),
            400,
        ),
        by_cid: cid,
        by: app
            .agents
            .get(cid)
            .map(|a| a.label())
            .unwrap_or_else(|| format!("agent {cid}")),
        created_ms: apz_plaza::now_ms(),
    };
    app.project_memory.add_proposal(proposal);
}

fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// A confirmed finding the author contested and the verifier held: the planner rules on it, on a model other than the
/// author's and the verifier's. Without a planner on another model, the owner rules (a `review` card).
fn request_ruling(app: &App, run: &Run, f: &Finding) {
    let author = run
        .models
        .iter()
        .find(|m| m.node == "author")
        .map(|m| m.model.clone())
        .unwrap_or_default();
    let verifier = f
        .verified_by
        .as_ref()
        .map(|b| b.model.clone())
        .unwrap_or_default();
    let planner =
        crate::work_loop::planner_cid(app, &run.project).map(|cid| model_use(app, "planner", cid));
    let on_other = planner
        .as_ref()
        .is_some_and(|p| !p.model.is_empty() && p.model != author && p.model != verifier);
    let text = format!(
        "Finding {} of review {} (task {}) is contested. The author says: {}. The verifier holds it: {}. Rule with finding_arbitrate (run {}, finding {}): fix, accept or drop, with your reason.",
        f.id, run.id, run.task, f.contest, f.proof, run.id, f.id
    );
    let now = apz_plaza::now_ms();
    if run.purpose == "seat" {
        if on_other {
            crate::work_loop::to_planner(app, &run.project, &text);
        }
        crate::work_loop::notice(
            app,
            &crate::work_loop::task_chat(app, &run.task, &run.project, now),
            &format!(
                "Task {} has a contested finding: {}.",
                run.task,
                if on_other {
                    "the planner rules on it"
                } else {
                    "the owner rules on it"
                }
            ),
            now,
        );
        return;
    }
    match planner {
        Some(_) if on_other => {
            crate::work_loop::to_planner(app, &run.project, &text);
            crate::work_loop::notice(
                app,
                &crate::work_loop::task_chat(app, &run.task, &run.project, now),
                &format!(
                    "Task {} has a contested finding: the planner rules on it.",
                    run.task
                ),
                now,
            );
        }
        _ => {
            app.reviews.update(&run.id, |r| {
                r.state = RunState::Owner;
                r.owner_asked = true;
            });
            if let Some(updated) = app.reviews.run(&run.id) {
                ask_owner(app, &updated);
            }
        }
    }
}

/// The author contests a confirmed finding of the review it was given in rework (`finding_contest`, 8.5): once, before
/// the author seals a newer revision. The finding goes back to the verifier, and the review reopens.
pub fn contest(app: &App, cid: u64, finding: &str, reason: &str) -> Result<String, String> {
    let item = app
        .work
        .current(cid)
        .ok_or("you are not working on a task")?;
    let task = item.origin.task.ok_or("your current work is not a task")?;
    let run = app
        .reviews
        .reworked_run_of(&task)
        .ok_or("the task has no review in rework to contest")?;
    let seat_path = run.purpose == "seat";
    let author = if seat_path {
        app.sessions.by_cid(cid).and_then(|s| s.seat).as_deref() == Some("builder")
    } else {
        run.worker == cid
    };
    if !author {
        return Err("only the author of the change contests a finding".into());
    }
    // A newer seal is reviewed as its own round: a contest comes before it (a review-graph run only; a seat's round
    // goes on from its rework and its verifier re-rules at the next round).
    let latest = if seat_path {
        None
    } else {
        app.weft
            .get(&format!("/changes/{}", run.change))
            .ok()
            .and_then(|d| {
                d["rounds"]
                    .as_array()
                    .and_then(|r| r.last().cloned())
                    .and_then(|r| r.as_str().map(str::to_string))
            })
    };
    if !seat_path && latest.as_deref() != Some(run.revision.as_str()) {
        return Err("the lane has a newer revision: contest before you seal it".into());
    }
    app.reviews
        .edit(&run.id, |r| contest_finding(r, finding, reason))?;
    if !seat_path {
        app.reviews.update(&run.id, reopen_review);
    }
    tick(app);
    Ok(format!("{finding} is contested: the verifier looks again"))
}

/// The planner's ruling on a contested finding (`finding_arbitrate`, 8.5): `fix`, `accept` or `drop`.
pub fn arbitrate(
    app: &App,
    cid: u64,
    run_id: &str,
    finding: &str,
    ruling: &str,
    reason: &str,
) -> Result<String, String> {
    let run = app.reviews.run(run_id).ok_or("no such review")?;
    let f = run
        .findings()
        .find(|f| f.id == finding)
        .cloned()
        .ok_or("no such finding")?;
    // The ruler is on a model other than the author's and the verifier's, or the owner rules.
    let me = model_use(app, "planner", cid).model;
    let author = run
        .models
        .iter()
        .find(|m| m.node == "author")
        .map(|m| m.model.clone())
        .unwrap_or_default();
    let verifier = f
        .verified_by
        .as_ref()
        .map(|b| b.model.clone())
        .unwrap_or_default();
    if me.is_empty() || me == author || me == verifier {
        return Err("a planner on a model other than the author's and the verifier's rules on it; without one, the owner does".into());
    }
    app.reviews
        .edit(run_id, |r| arbitrate_finding(r, finding, ruling, reason))?;
    let now = apz_plaza::now_ms();
    crate::work_loop::notice(
        app,
        &crate::work_loop::task_chat(app, &run.task, &run.project, now),
        &format!(
            "The planner ruled {ruling} on {finding} of task {}: {reason}",
            run.task
        ),
        now,
    );
    tick(app);
    Ok(format!("{finding}: {ruling}"))
}

// ── the seat path: a task's review round on the work loop's seats (8.2) ─────────────────────────────────────────────

/// The latest revision of `change` Weft has sealed.
pub fn latest_revision(app: &App, change: &str) -> String {
    app.weft
        .get(&format!("/changes/{change}"))
        .ok()
        .and_then(|d| {
            d["rounds"]
                .as_array()
                .and_then(|r| r.last().cloned())
                .and_then(|r| r.as_str().map(str::to_string))
        })
        .unwrap_or_default()
}

/// A review round on the seats starts (a reviewer seat is about to run): the task's review run is made for it, or,
/// after a rework, reopened for the next round. Its findings stay; its review nodes wait again. Idempotent within a
/// round: the first reviewer to start opens it, the others find it open.
pub fn open_round(app: &App, task: &str) {
    let Some(record) = app.plaza.tasks.find(task) else {
        return;
    };
    let Some(project) = record.project().map(str::to_string) else {
        return;
    };
    let worker = record.view().agent_cid.unwrap_or(0);
    let Some(change) = app.weft.lane_of(task) else {
        return;
    };
    let revision = latest_revision(app, &change);
    let seats = app.seats.of_task(task);
    let author = seats
        .iter()
        .find(|s| s.role == "builder")
        .and_then(|s| s.last_model.clone())
        .unwrap_or_default();
    let defs: Vec<NodeDef> = seats
        .iter()
        .filter(|s| s.role == "reviewer" || s.role == "verifier")
        .map(|s| NodeDef {
            id: s.name.clone(),
            kind: "agent".into(),
            role: Some(s.role.clone()),
            ..Default::default()
        })
        .collect();
    if defs.is_empty() {
        return;
    }
    let author_use = ModelUse {
        node: "author".into(),
        cid: worker,
        model: author,
        session: String::new(),
    };
    let now = apz_plaza::now_ms();
    app.reviews.with(|b| {
        let open = b
            .runs
            .values()
            .filter(|r| r.task == task && r.purpose == "seat")
            .max_by_key(|r| r.created_ms)
            .map(|r| r.id.clone());
        match open {
            Some(id) => {
                let Some(r) = b.runs.get_mut(&id) else {
                    return;
                };
                if r.state != RunState::Rework {
                    return;
                }
                r.state = RunState::Running;
                r.round += 1;
                r.revision = revision;
                r.worker = worker;
                for def in defs {
                    if r.node(&def.id).is_none() {
                        r.nodes.push(NodeRun {
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
                        });
                    } else if let Some(n) = r.node_mut(&def.id) {
                        n.state = NodeState::Waiting;
                        n.assignee = None;
                        n.work = None;
                        n.tries = 0;
                        n.notes.clear();
                        n.ended_ms = 0;
                    }
                }
                r.models.retain(|m| m.node != "author");
                r.models.push(author_use);
                r.updated_ms = now;
            }
            None => {
                b.next += 1;
                let id = format!("rv{}", b.next);
                let nodes = defs
                    .into_iter()
                    .map(|def| NodeRun {
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
                    })
                    .collect();
                b.runs.insert(
                    id.clone(),
                    Run {
                        id,
                        task: task.into(),
                        project,
                        worker,
                        change,
                        revision,
                        recipe: "change@2".into(),
                        round: 1,
                        rounds: 3,
                        kind: "code".into(),
                        state: RunState::Running,
                        nodes,
                        rules: vec![],
                        owner_asked: false,
                        outcome: None,
                        tree_blob: None,
                        summary: String::new(),
                        purpose: "seat".into(),
                        repo: String::new(),
                        models: vec![author_use],
                        created_ms: now,
                        updated_ms: now,
                    },
                );
            }
        }
    });
}

/// A review seat's session started on `cid` with `model`: its node is its (the review's reviewer or verifier).
pub fn seat_started(app: &App, task: &str, seat: &str, cid: u64, model: &str, session: &str) {
    let Some(run) = app.reviews.seat_run(task) else {
        return;
    };
    app.reviews.update(&run.id, |r| {
        if let Some(n) = r.node_mut(seat) {
            n.state = NodeState::Running;
            n.assignee = Some(cid);
            n.started_ms = apz_plaza::now_ms();
            n.tries += 1;
        }
        r.models.retain(|m| m.node != seat);
        r.models.push(ModelUse {
            node: seat.into(),
            cid,
            model: model.into(),
            session: session.into(),
        });
    });
}

/// Whether the seat path's round is on a model other than its author's, every reviewer's and the verifier's: the gap,
/// named (the same rule as the review graph's gate, see [`cross_model_gap`]).
pub fn seat_gap(app: &App, task: &str) -> Option<String> {
    cross_model_gap(&app.reviews.seat_run(task)?)
}

/// The walk moved on: a rewind reopens the round for rework, and the walk's end closes the review run.
pub fn seat_settled(app: &App, task: &str, moves: &[crate::recipes::Move]) {
    use crate::recipes::Move;
    let state = moves.iter().find_map(|m| match m {
        Move::Rewind(_) => Some(RunState::Rework),
        Move::Done => Some(RunState::Passed),
        Move::Failed(_) => Some(RunState::Failed),
        _ => None,
    });
    let (Some(state), Some(run)) = (state, app.reviews.seat_run(task)) else {
        return;
    };
    app.reviews.update(&run.id, |r| r.state = state);
}

/// The confirmed findings of the seat round's current round: the count the verifier's step reports to the walk.
pub fn seat_findings(app: &App, task: &str) -> u32 {
    app.reviews
        .seat_run(task)
        .map(|r| {
            r.findings()
                .filter(|f| {
                    f.verdict == Verdict::Confirmed && f.severity != "info" && f.round == r.round
                })
                .count() as u32
        })
        .unwrap_or(0)
}

/// What the builder is sent back with: the round's confirmed findings, and the dropped ones with their reasons.
pub fn seat_said(app: &App, task: &str) -> String {
    let Some(run) = app.reviews.seat_run(task) else {
        return String::new();
    };
    let (confirmed, dropped) = findings_text(&run);
    let mut out = format!(
        "The verifier confirmed these findings (fix them, each with its scenario in mind):\n{}",
        confirmed.join("\n")
    );
    if !dropped.is_empty() {
        out.push_str(&format!(
            "\nDropped (not defects, nothing to change):\n{}",
            dropped.join("\n")
        ));
    }
    out
}

/// The findings a verifier has yet to rule on (its verdict is refused while any is open).
pub fn pending_findings(app: &App, task: &str) -> Vec<String> {
    app.reviews
        .seat_run(task)
        .map(|r| {
            r.findings()
                .filter(|f| f.verdict == Verdict::Pending)
                .map(|f| f.id.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// A review seat's standing instructions in its prompt: a reviewer's posture and the project's pitfalls, or a
/// verifier's findings to rule on.
pub fn seat_brief(app: &App, task: &str, role: &str) -> String {
    if role == "verifier" {
        return app
            .reviews
            .seat_run(task)
            .map(|r| verifier_text(&r))
            .unwrap_or_default();
    }
    let project = app
        .plaza
        .tasks
        .find(task)
        .and_then(|t| t.project().map(str::to_string))
        .unwrap_or_default();
    let mut p = sceptic_text(app, &project);
    p.push_str("Record each finding with review_score: its findings (symbol, text, severity, and the scenario). Then plaza_verdict with pass true once you have reviewed the change, or false when you cannot review it.\n");
    p
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
        let avoid = avoid_models(&r, &n.def);
        if let Some(cid) = pick_agent(app, &r.project, &role, r.worker, &taken, &avoid)
            .or_else(|| pick_agent(app, &r.project, &role, r.worker, &[], &avoid))
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
            Condition::ConfirmedFindings => {
                let confirmed: Vec<&str> = run
                    .findings()
                    .filter(|f| f.verdict == Verdict::Confirmed && f.severity != "info")
                    .map(|f| f.id.as_str())
                    .collect();
                (!confirmed.is_empty()).then(|| {
                    format!(
                        "{} confirmed finding(s): {}",
                        confirmed.len(),
                        confirmed.join(", ")
                    )
                })
            }
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
    } else if let Some(gap) = cross_model_gap(run) {
        // A pass needs reviewers on another model than the author's: with none, the owner rules (and says so).
        ("owner", vec![format!("no review on another model: {gap}")])
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
    app.recipe_runs.finish_subject(
        "review",
        &run.id,
        &crate::recipes::Report::verdict(outcome.decision == "pass"),
        &outcome.decision,
        outcome.decision == "owner",
        apz_plaza::now_ms(),
    );
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
        // The lane sealed a newer revision while this run went on: only the one reviewed is admitted, and the newer
        // one gets its own run (the scheduler starts it once this one ends).
        let latest = d["rounds"]
            .as_array()
            .and_then(|r| r.last())
            .and_then(|r| r.as_str());
        if run.purpose == "review" && latest.is_some_and(|l| l != run.revision) {
            eprintln!(
                "apid: {} passed on {}, but the lane has moved on: not queued",
                run.id, run.revision
            );
            return;
        }
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
    let (findings, dropped) = findings_text(run);
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
        "{goal}\n\n{note}\n\nWhat the review found (confirmed by its verifier):\n{found}{dropped}\n\nFix the confirmed findings in your lane{lane} and finish again; the next review round reads your next revision. A confirmed finding you think is wrong can be contested once, with finding_contest (its id and your reason): the verifier looks again.",
        goal = task.goal(),
        dropped = if dropped.is_empty() {
            String::new()
        } else {
            format!(
                "\n\nDropped (not defects, nothing to change):\n{}",
                dropped.join("\n")
            )
        },
        found = if findings.is_empty() {
            "- (no confirmed findings beyond the reasons above)".into()
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
    if item.detail.get("gate").and_then(Value::as_str) == Some("cross_model") {
        // The reviewer ran on the builder's own model: the owner admits the change or fails the review (8.2).
        let task = item.detail["task"].as_str().ok_or("no task on the card")?;
        let now = apz_plaza::now_ms();
        if approve {
            crate::work_loop::admit(app, task, now);
            return Ok(());
        }
        let notes = item
            .reason
            .clone()
            .unwrap_or_else(|| "the owner did not admit the review".into());
        return app.work.owner_verdict(app, task, false, &notes).map(|_| ());
    }
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
    app.recipe_runs.finish_subject(
        "review",
        &run.id,
        &crate::recipes::Report::verdict(decision == "pass"),
        decision,
        false,
        apz_plaza::now_ms(),
    );
    match decision {
        "pass" => passed(app, &run, &notes),
        "rework" => send_back(app, &run, std::slice::from_ref(&notes)),
        _ => failed(app, &run, &notes),
    }
    save_owner_decision(app);
    app.bus.publish(
        "review_run",
        &json!({ "run": run.id, "task": run.task, "state": decision, "by": "owner" }),
    );
    Ok(())
}

/// Saves the book now: an owner's answer (a ruling, a recipe, a proposal) returns once it is committed. A failed save
/// is retried by the next pass, so it is only logged here.
pub fn save_owner_decision(app: &App) {
    if !crate::store::saver::save_now(app, "review_graph") {
        eprintln!(
            "apid: the review graph's owner decision is not saved yet (the next pass retries)"
        );
    }
}

/// Findings of earlier rounds on definitions still in this round's tree (they follow the definition's lineage).
pub fn carried_findings(app: &App, run: &Run) -> Vec<Finding> {
    let tree = app.reviews.tree(run);
    let lineages: Vec<String> = tree["files"]
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
                tree_blob: None,
                summary: String::new(),
                purpose: purpose.into(),
                repo: repo.into(),
                models: vec![],
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
            tree_blob: None,
            summary: String::new(),
            purpose: "review".into(),
            repo: String::new(),
            models: vec![],
            created_ms: 0,
            updated_ms: 0,
        }
    }

    /// A review graph over an object store in `dir` (which the test keeps alive).
    fn reviews_in(dir: &tempfile::TempDir) -> Reviews {
        let blobs = Blobs::open(&dir.path().join("blobs")).expect("the object store opens");
        Reviews::new(Arc::new(blobs))
    }

    #[test]
    fn of_ticks_that_start_one_node_at_once_only_one_takes_it() {
        let dir = tempfile::tempdir().unwrap();
        let reviews = Arc::new(reviews_in(&dir));
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
        let dir = tempfile::tempdir().unwrap();
        let reviews = reviews_in(&dir);
        reviews.with(|b| b.runs.insert("rv1".into(), run_with(RunState::Rework)));
        assert!(!reviews.claim("rv1", "quality"));
        assert!(!reviews.claim("rv2", "quality"));
        assert_eq!(
            reviews.run("rv1").unwrap().node("quality").unwrap().state,
            NodeState::Waiting
        );
    }

    #[test]
    fn the_book_is_kept_in_its_tables_with_the_tree_in_the_object_store() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = Arc::new(Blobs::open(&dir.path().join("blobs")).unwrap());
        let reviews = Reviews::new(Arc::clone(&blobs));
        let tree = json!({ "files": [{ "path": "src/lib.rs", "symbols": [{ "id": "a", "lineage": "L1" }] }] });
        let mut run = run_with(RunState::Running);
        run.tree_blob = Some(reviews.put_tree(&tree).unwrap());
        reviews.with(|b| {
            b.next = 7;
            b.runs.insert("rv1".into(), run);
        });
        let conn = apz_store::rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&def().ddl()).unwrap();
        apz_store::table::replace(&conn, def(), &reviews.rows().unwrap()).unwrap();

        let fresh = Reviews::new(blobs);
        fresh.load(&conn).unwrap();
        assert_eq!(fresh.with(|b| b.next), 7);
        let back = fresh.run("rv1").unwrap();
        assert_eq!(
            format!("{back:?}"),
            format!("{:?}", reviews.run("rv1").unwrap())
        );
        assert_eq!(fresh.tree(&back), tree);
    }

    fn agent_node(id: &str, role: &str, axes: &[&str]) -> NodeRun {
        NodeRun {
            def: NodeDef {
                id: id.into(),
                kind: "agent".into(),
                role: Some(role.into()),
                axes: axes.iter().map(|a| a.to_string()).collect(),
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
        }
    }

    fn finding(symbol: &str, scenario: &str) -> Finding {
        Finding {
            symbol: symbol.into(),
            lineage: None,
            text: format!("{symbol} is wrong"),
            severity: "major".into(),
            node: "function".into(),
            round: 1,
            id: String::new(),
            scenario: scenario.into(),
            kind: "tst".into(),
            verdict: Verdict::Pending,
            proof: String::new(),
            reason: String::new(),
            by: None,
            verified_by: None,
            contest: String::new(),
            ruling: String::new(),
            ruling_reason: String::new(),
        }
    }

    fn model(node: &str, cid: u64, model: &str) -> ModelUse {
        ModelUse {
            node: node.into(),
            cid,
            model: model.into(),
            session: String::new(),
        }
    }

    fn by(seat: &str, model: &str) -> By {
        By {
            seat: seat.into(),
            model: model.into(),
            session: format!("s-{seat}"),
        }
    }

    /// A code review's shape: a reviewer, the verifier after it, and the gate; the models of the author, the reviewer
    /// and the verifier are given in that order.
    fn chain_run(round: u32, models: [&str; 3]) -> Run {
        let mut run = run_with(RunState::Running);
        run.round = round;
        run.rounds = 3;
        let mut gate = agent_node("gate", "reviewer", &[]);
        gate.def.kind = "gate".into();
        gate.def.role = None;
        gate.def.after = vec!["function".into(), "verify".into()];
        run.nodes = vec![
            agent_node("function", "reviewer", &["fea"]),
            agent_node("verify", "verifier", &[]),
            gate,
        ];
        run.models = vec![
            model("author", 300, models[0]),
            model("function", 301, models[1]),
            model("verify", 303, models[2]),
        ];
        run.rules = vec![
            recipe::Rule {
                when: "confirmed findings".into(),
                then: "rework".into(),
            },
            recipe::Rule {
                when: "failed".into(),
                then: "rework".into(),
            },
        ];
        run
    }

    /// Records findings the way `report` numbers them: `f1`, `f2`, … in the run's order, pending.
    fn record(run: &mut Run, node: &str, findings: Vec<Finding>) {
        let base: usize = run.nodes.iter().map(|n| n.findings.len()).sum();
        let n = run.nodes.iter_mut().find(|n| n.def.id == node).unwrap();
        for (i, mut f) in findings.into_iter().enumerate() {
            f.id = format!("f{}", base + i + 1);
            f.by = Some(by(node, "mock/echo"));
            n.findings.push(f);
        }
    }

    #[test]
    fn a_finding_without_its_scenario_is_refused() {
        assert!(check_findings(&[finding("q.rs::next", "")]).is_err());
        assert!(check_findings(&[finding("q.rs::next", "   ")]).is_err());
        assert!(check_findings(&[finding("q.rs::next", "an empty queue panics")]).is_ok());
    }

    #[test]
    fn the_seeded_bug_is_proved_and_fixed_and_the_false_alarm_is_dropped_with_its_reason() {
        let verifier = by("verify", "mock/third");
        // Round 1: the reviewer (on another model than the author's) finds the seeded bug and a false alarm.
        let mut r1 = chain_run(1, ["mock/agent", "mock/echo", "mock/third"]);
        record(
            &mut r1,
            "function",
            vec![
                finding("q.rs::Queue::next", "an empty queue → next() panics"),
                finding("q.rs::Queue::next", "items come out newest first"),
            ],
        );
        // The verifier proves the bug with a test it ran, and drops the false alarm with the reason.
        rule_finding(
            &mut r1,
            "f1",
            Verdict::Confirmed,
            "test next_on_empty: Queue { items: vec![] }.next() panics",
            "",
            verifier.clone(),
        )
        .unwrap();
        rule_finding(
            &mut r1,
            "f2",
            Verdict::Dropped,
            "",
            "the test shows the oldest item first: pop() on a vector of pushes is the oldest",
            verifier.clone(),
        )
        .unwrap();
        assert!(
            rule_finding(&mut r1, "f1", Verdict::Dropped, "", "no", verifier.clone()).is_err(),
            "a ruling is made once"
        );
        let f2 = r1.findings().find(|f| f.id == "f2").unwrap();
        assert_eq!(f2.verdict, Verdict::Dropped);
        assert!(f2.reason.contains("oldest"));
        assert_eq!(f2.by.as_ref().unwrap().seat, "function");
        assert_eq!(f2.verified_by.as_ref().unwrap().model, "mock/third");
        // The confirmed bug sends the author back with its scenario; the false alarm does not.
        let out = decide(&r1);
        assert_eq!(out.decision, "rework", "{:?}", out.reasons);
        assert!(out.reasons.join(" ").contains("f1"));

        // Round 2: the author fixed it, the chain re-ran on the new revision, and nothing confirmed is left.
        let r2 = chain_run(2, ["mock/agent", "mock/echo", "mock/third"]);
        assert_eq!(decide(&r2).decision, "pass");
    }

    #[test]
    fn a_verifier_confirms_with_a_proof_and_drops_with_a_reason_and_no_other_way() {
        let mut run = chain_run(1, ["mock/agent", "mock/echo", "mock/third"]);
        record(&mut run, "function", vec![finding("q.rs::next", "a state")]);
        assert!(
            rule_finding(
                &mut run,
                "f1",
                Verdict::Confirmed,
                "",
                "",
                by("verify", "m")
            )
            .is_err()
        );
        assert!(rule_finding(&mut run, "f1", Verdict::Dropped, "", "", by("verify", "m")).is_err());
        assert!(
            rule_finding(
                &mut run,
                "f1",
                Verdict::Pending,
                "x",
                "y",
                by("verify", "m")
            )
            .is_err()
        );
        assert!(
            rule_finding(&mut run, "f9", Verdict::Dropped, "", "r", by("verify", "m")).is_err()
        );
    }

    #[test]
    fn a_contest_is_once_and_the_verifier_holds_it_for_the_planner_to_rule() {
        let verifier = by("verify", "mock/third");
        let mut run = chain_run(1, ["mock/agent", "mock/echo", "mock/third"]);
        record(
            &mut run,
            "function",
            vec![finding("q.rs::next", "an empty queue panics")],
        );
        rule_finding(
            &mut run,
            "f1",
            Verdict::Confirmed,
            "a failing test",
            "",
            verifier.clone(),
        )
        .unwrap();
        run.state = RunState::Rework;

        // The author contests the confirmed bug: the review reopens for the verifier's second look.
        contest_finding(&mut run, "f1", "the queue is never empty at this call").unwrap();
        reopen_review(&mut run);
        assert_eq!(run.state, RunState::Running);
        assert!(run.node("verify").unwrap().state == NodeState::Waiting);
        assert!(contest_finding(&mut run, "f1", "again").is_err(), "once");

        // The verifier holds it: the planner rules, and the gate waits for the ruling.
        rule_finding(
            &mut run,
            "f1",
            Verdict::Confirmed,
            "the same test, on the call",
            "",
            verifier.clone(),
        )
        .unwrap();
        assert!(run.awaiting_ruling());
        assert!(arbitrate_finding(&mut run, "f1", "maybe", "x").is_err());
        arbitrate_finding(&mut run, "f1", "fix", "the test is right").unwrap();
        assert!(!run.awaiting_ruling());
        assert_eq!(decide(&run).decision, "rework");
    }

    #[test]
    fn a_contested_finding_the_verifier_drops_is_dropped_and_needs_no_planner() {
        let verifier = by("verify", "mock/third");
        let mut run = chain_run(1, ["mock/agent", "mock/echo", "mock/third"]);
        record(&mut run, "function", vec![finding("q.rs::next", "a state")]);
        rule_finding(
            &mut run,
            "f1",
            Verdict::Confirmed,
            "a test",
            "",
            verifier.clone(),
        )
        .unwrap();
        contest_finding(&mut run, "f1", "the state cannot occur").unwrap();
        rule_finding(
            &mut run,
            "f1",
            Verdict::Dropped,
            "",
            "the state cannot occur: checked",
            verifier,
        )
        .unwrap();
        assert!(!run.awaiting_ruling());
        assert_eq!(decide(&run).decision, "pass");
    }

    #[test]
    fn a_pass_needs_reviewers_and_the_verifier_on_models_other_than_the_authors() {
        let other = ["mock/agent", "mock/echo", "mock/third"];
        assert_eq!(decide(&chain_run(1, other)).decision, "pass");
        assert_eq!(cross_model_gap(&chain_run(1, other)), None);

        let same_reviewer = chain_run(1, ["mock/agent", "mock/agent", "mock/third"]);
        let out = decide(&same_reviewer);
        assert_eq!(out.decision, "owner");
        assert!(
            out.reasons[0].contains("the author's own model"),
            "{:?}",
            out.reasons
        );
        assert_eq!(
            decide(&chain_run(1, ["mock/agent", "mock/echo", "mock/agent"])).decision,
            "owner"
        );

        // A run without its author's model is not admitted (it fails closed).
        let mut unknown = chain_run(1, other);
        unknown.models[0].model = String::new();
        assert_eq!(decide(&unknown).decision, "owner");
        let mut none = chain_run(1, other);
        none.models.retain(|m| m.node != "author");
        assert!(cross_model_gap(&none).is_some());
    }

    #[test]
    fn a_kind_confirmed_twice_in_a_project_is_counted_for_its_pitfall() {
        let dir = tempfile::tempdir().unwrap();
        let reviews = reviews_in(&dir);
        let confirmed = |id: &str, project: &str| {
            let mut run = chain_run(1, ["mock/agent", "mock/echo", "mock/third"]);
            run.id = id.into();
            run.project = project.into();
            record(&mut run, "function", vec![finding("q.rs::next", "s")]);
            rule_finding(
                &mut run,
                "f1",
                Verdict::Confirmed,
                "p",
                "",
                by("verify", "m"),
            )
            .unwrap();
            reviews.with(|b| b.runs.insert(id.into(), run));
        };
        confirmed("rv1", "p1");
        assert_eq!(reviews.confirmed_of_kind("p1", "tst"), 1);
        confirmed("rv2", "p1");
        assert_eq!(reviews.confirmed_of_kind("p1", "tst"), 2);
        assert_eq!(reviews.confirmed_of_kind("p2", "tst"), 0);
    }
}
