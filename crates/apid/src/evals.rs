//! Eval records (PLAN 25.1): every step session that ends leaves one `eval_run` row, what the eval suite and the owner's
//! scores read. The row is written through to `plaza.db` as the session ends ([`ended`]). What the session cost comes
//! from its record (usage, model, node, verdict); its tool time and its memory reads come from its VM, which the hub adds
//! up as they arrive ([`Evals::tool`]). The later outcomes are set when they happen: the task's change lands
//! ([`Evals::landed`]), is reverted ([`Evals::reverted`]), draws an incident ([`Evals::incident`]), has a finding confirmed
//! ([`Evals::finding`]), or passes qualification in the lab ([`Evals::qualified`]).
//!
//! The reports (`report.rs`) fold the rows: costs by project, task, step or model, and each reviewer model's precision
//! and misses. The recipe benchmark replays an archived task: the replay is a task in its project, run through the work
//! loop like any other, and when it lands or fails it is scored against the case's known outcome ([`Evals::replay_finished`]).
//!
//! GET /v1/evals/runs?project=&model=&seat=&limit=  → [`Run`]s, newest first (limit 1..=200, default 50)
//! GET /v1/evals/scores                             → [`Score`]s per step, seat, model and engine
//! GET /v1/evals/leaderboard?step=                  → the scores of a step (or all), best first
//! GET /v1/evals/costs?by=project|task|step|model&project=&model=&seat= → [`CostRow`]s, the most expensive first
//! GET /v1/evals/reviewers                          → [`ReviewerScore`]s: each reviewer model's precision and misses
//! GET /v1/evals/archive                            → the archived tasks with a known outcome (the benchmark's cases)
//! GET /v1/evals/bench                              → the benchmark's replays, newest first
//! POST /v1/evals/bench/{id}                        → replays archived case `id` (a task in its project; 201 with the task)
//! GET /v1/evals/tools                              → [`ToolLatency`] per tool, over the newest tool calls kept

pub mod choose;
pub mod report;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use apz_http::{Request, Response, RouteDoc, Router};
use apz_store::rusqlite::{Connection, OptionalExtension, Row, params};
use apz_store::table::Value;
use apz_store::{RowSet, StoreError, Table, TableDef};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::sessions::{Kind, Session};
use crate::state::App;
use crate::store::sql;

pub use report::{By, CostRow, Pass, ReviewerScore};

/// The newest runs a page returns by default, and the most one request may ask for.
pub const RUNS_PAGE: usize = 50;
pub const RUNS_LIMIT: usize = 200;
/// The newest tool calls whose durations the hub keeps (in memory: the latencies are for the hub's current life).
pub const TOOL_CALLS_KEPT: usize = 50_000;
/// The recipe version a change seat runs under when its session was never begun (the `change` recipe is version 2).
const CHANGE_VERSION: u32 = 2;

/// One step session's record (a row of `eval_run`). The key is the session's number, so a session's record is written
/// once and its later outcomes update it in place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "eval_run")]
pub struct Run {
    /// The session's number (`s<id>`'s number).
    #[table(key)]
    pub id: u64,
    /// The session's id (`s<id>`).
    pub session: String,
    #[table(index)]
    pub project: String,
    #[table(index)]
    pub task: Option<String>,
    /// The recipe the seat belongs to (`change`).
    pub recipe: String,
    /// The recipe's step the seat runs (`build`, `review`).
    pub step: String,
    /// The seat (`builder`, `reviewer`).
    #[table(index)]
    pub seat: String,
    /// The seat's loop round (1 at the start; a failed review starts round 2 for the builder).
    pub round: u32,
    /// The engine that ran the session (`harness`) and its version (the hub's own).
    pub engine: String,
    pub engine_version: String,
    /// The model the session ran, `<provider>/<model>`, and its provider.
    #[table(index)]
    pub model: Option<String>,
    pub provider: Option<String>,
    /// The harness's capabilities as its VM said hello (`caps`, comma-separated).
    pub harness: String,
    pub node: Option<String>,
    /// The pool the node was in when the session ended (`arch/hypervisor/cpu_baseline`, `nodes::Pool::key`); None when
    /// it had none (plan 7.5: a test result or an eval says where it ran).
    pub pool: Option<String>,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub tokens_cached: u64,
    /// Micro-dollars.
    pub cost_micros: u64,
    pub model_calls: u64,
    pub tool_calls: u64,
    /// The time the session's tools ran, in milliseconds (the harness's own measure of each call).
    pub tool_ms: u64,
    /// From the session's start (or its creation, when it never started) to its end, in milliseconds.
    pub wall_ms: u64,
    /// The reviewer's verdict on the step, when it gave one (true: passed).
    pub verdict: Option<bool>,
    /// How the session ended (the session's outcome).
    pub outcome: String,
    /// Whether the step succeeded: it ended without failing, and no reviewer rejected it.
    pub ok: bool,
    /// Whether the task's change landed, once known.
    pub landed: Option<bool>,
    /// Whether the landed change was reverted afterwards, once known.
    pub reverted: Option<bool>,
    #[table(index)]
    pub ended_ms: u64,
    /// The recipe's version the seat ran under (`change` is version 2).
    pub recipe_version: u32,
    /// The harness's version, when its hello says it (a `harness=<version>` capability; empty when it sends none).
    pub harness_version: String,
    /// The gate's result: `passed` (the reviewer passed the step), `failed` (a reviewer rejected it, or the session
    /// failed), or `none` (the step ended and no reviewer gates it).
    pub gate: String,
    /// Findings confirmed later on the task (the builder's runs carry them: a finding is a defect in what they built).
    pub later_findings: u64,
    /// Incidents traced back to the task's landing.
    pub incidents: u64,
    /// Memory entries the session read (its `memory_get` calls).
    pub memory_cites: u64,
    /// Qualification stages the task's change passed in the lab, and the lab minutes they took.
    pub qual_stages: u64,
    pub lab_minutes: u64,
    /// How the session's model (and engine) was chosen: `fixed`, `list`, `seat`, or for a step with `model: eval` or
    /// `engine: any` the rule (`eval:exploited`, `eval:explored`, `eval:untrusted`, `eval:only`), see `choose.rs`.
    /// Empty when the run was never begun by a recipe step.
    #[serde(default)]
    pub choice: String,
    /// The reason of the choice in words: the scores and samples it compared, and what the budget skipped.
    #[serde(default)]
    pub choice_reason: String,
}

/// Which runs to list. Each field is a match, and None matches any.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunFilter {
    pub project: Option<String>,
    pub model: Option<String>,
    pub seat: Option<String>,
}

/// How the runs of one step, seat, model and engine did.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Score {
    pub step: String,
    pub seat: String,
    pub model: Option<String>,
    pub engine: String,
    pub runs: u64,
    pub successes: u64,
    /// Successes over runs, 0 to 1.
    pub success_rate: f64,
    /// Micro-dollars spent over all the runs, per success; None without a success.
    pub cost_per_success_micros: Option<f64>,
    /// Milliseconds from start to end over all the runs, per success; None without a success.
    pub time_per_success_ms: Option<f64>,
    /// Clean successes over runs, 0 to 1 (see [`report::clean`]).
    pub quality: f64,
    /// Micro-dollars over all the runs (the sum the cost per success divides).
    #[serde(default)]
    pub cost_micros: u64,
    /// Milliseconds from start to end over all the runs (the sum the time per success divides).
    #[serde(default)]
    pub wall_ms: u64,
}

/// One finding a review ruled on (a row of `eval_finding`): who raised it, who ruled on it, and how it was ruled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "eval_finding")]
pub struct FindingRecord {
    /// A counter, in the order the rulings were recorded.
    #[table(key)]
    pub id: u64,
    #[table(index)]
    pub project: String,
    #[table(index)]
    pub task: Option<String>,
    /// The review run the finding belongs to, and the finding's id in it.
    pub review: String,
    pub finding: String,
    /// The reviewer model that raised it, and its seat.
    #[table(index)]
    pub reviewer: String,
    pub reviewer_seat: String,
    /// The verifier's model (empty when none ruled).
    pub verifier: String,
    /// `confirmed` or `dropped`.
    pub verdict: String,
    pub severity: String,
    #[table(index)]
    pub ruled_ms: u64,
}

/// A lab run's qualification (a row of `eval_qualification`), keyed by the run's number: a run is recorded once, however
/// often its stages are judged again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "eval_qualification")]
pub struct Qualification {
    #[table(key)]
    pub id: u64,
    #[table(index)]
    pub task: String,
    pub stages: u64,
    pub lab_minutes: u64,
}

/// The qualifications' table (`eval_qualification`), written through.
pub fn qualification_def() -> &'static TableDef {
    apz_store::root_def!(Qualification)
}

/// An archived task with a known outcome (the benchmark's cases, PLAN 25.3, kept forever: W32): the commit it started
/// from, its goal, the check that tells whether its change works, and whether the change landed or was reverted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "eval_archive")]
pub struct Archived {
    #[table(key)]
    pub id: u64,
    #[table(index)]
    pub project: String,
    pub task: String,
    /// The commit the task started from.
    pub start: String,
    pub goal: String,
    /// The acceptance the replay's task is judged by (the task's own acceptance text).
    pub check: String,
    /// A test command the replay's builder runs in its lane with its `run` tool. A replay that names one passes only when
    /// its last run of it passed; empty when the case names none (the replay then passes when its change lands).
    #[serde(default)]
    pub command: String,
    /// `landed` (the change stood) or `reverted` (it was undone): what the replay's check must agree with.
    pub outcome: String,
    pub archived_ms: u64,
}

/// One replay of an archived task (a row of `eval_bench`): whether its check passed, what the known outcome says, and
/// whether the two agree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, apz_store::Table)]
#[table(name = "eval_bench")]
pub struct BenchRun {
    #[table(key)]
    pub id: u64,
    #[table(index)]
    pub archive: u64,
    /// The replay task the route opened in the case's project.
    #[table(index)]
    pub task: Option<String>,
    /// The Weft branch the replay's lane lands to: created at the case's start commit, never merged into main.
    pub reference: String,
    /// The test command the replay runs (empty for none), and whether its last run passed (None: it did not run).
    pub command: String,
    pub check: Option<bool>,
    /// `passed` or `failed`: whether the replay's check passed.
    pub outcome: String,
    pub known: String,
    pub matched: bool,
    pub cost_micros: u64,
    pub wall_ms: u64,
    pub replayed_ms: u64,
}

/// How one tool did over the calls the hub keeps: how many ran, how many failed, and the time they took (nearest rank).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolLatency {
    pub tool: String,
    pub calls: u64,
    pub errors: u64,
    pub p50_ms: u64,
    pub p95_ms: u64,
}

/// One tool call a VM reported.
#[derive(Debug, Clone, PartialEq)]
struct Call {
    tool: String,
    ms: u64,
    ok: bool,
}

/// What a session's record needs from the moment it starts and from its VM, kept until it ends.
#[derive(Debug, Clone, Default)]
struct Begun {
    recipe: String,
    version: u32,
    step: String,
    round: u32,
    choice: String,
    choice_reason: String,
}

/// What the hub knows of a VM while its session runs: the capabilities it said hello with, its harness version, its
/// tools' time and its memory reads.
#[derive(Debug, Clone, Default)]
struct Vm {
    harness: String,
    harness_version: String,
    tool_ms: u64,
    memory_cites: u64,
    /// The `run` commands the VM reported, in order, and whether each passed.
    runs: Vec<(String, bool)>,
}

/// The eval records: writes runs through `plaza.db`, reads them back, and keeps what a running session has so far.
pub struct Evals {
    db: Arc<apz_store::Db>,
    begun: Mutex<HashMap<String, Begun>>,
    vms: Mutex<HashMap<u64, Vm>>,
    /// The newest tool calls, up to [`TOOL_CALLS_KEPT`].
    calls: Mutex<VecDeque<Call>>,
}

impl Evals {
    /// Records over `db` (the hub's `plaza.db`), with nothing in flight.
    pub fn new(db: Arc<apz_store::Db>) -> Evals {
        Evals {
            db,
            begun: Mutex::new(HashMap::new()),
            vms: Mutex::new(HashMap::new()),
            calls: Mutex::new(VecDeque::new()),
        }
    }

    fn begun(&self) -> MutexGuard<'_, HashMap<String, Begun>> {
        self.begun.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn vms(&self) -> MutexGuard<'_, HashMap<u64, Vm>> {
        self.vms.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A session started for a seat of `recipe` (its version `version`) running `step` in its loop `round`: its record
    /// keeps them.
    pub fn begin(&self, session: &str, recipe: &str, version: u32, step: &str, round: u32) {
        self.begun().insert(
            session.to_string(),
            Begun {
                recipe: recipe.to_string(),
                version,
                step: step.to_string(),
                round,
                ..Begun::default()
            },
        );
    }

    /// How the session begun under `session` got its model and engine (`choice`, its reason): the record keeps them.
    pub fn decided(&self, session: &str, choice: &str, reason: &str) {
        if let Some(begun) = self.begun().get_mut(session) {
            begun.choice = choice.to_string();
            begun.choice_reason = reason.to_string();
        }
    }

    /// VM `cid` said hello with `caps`. A `harness=<version>` capability is the harness's version.
    pub fn hello(&self, cid: u64, caps: &[String]) {
        let mut vms = self.vms();
        let vm = vms.entry(cid).or_default();
        vm.harness = caps.join(",");
        vm.harness_version = caps
            .iter()
            .find_map(|cap| cap.strip_prefix("harness="))
            .unwrap_or_default()
            .to_string();
    }

    /// A tool of VM `cid` ran for `ms` milliseconds, and did (`ok`) or failed. A memory read counts as a citation.
    pub fn tool(&self, cid: u64, tool: &str, ms: u64, ok: bool) {
        {
            let mut vms = self.vms();
            let vm = vms.entry(cid).or_default();
            vm.tool_ms += ms;
            if tool == "memory_get" {
                vm.memory_cites += 1;
            }
        }
        let mut calls = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
        if calls.len() == TOOL_CALLS_KEPT {
            calls.pop_front();
        }
        calls.push_back(Call {
            tool: tool.to_string(),
            ms,
            ok,
        });
    }

    /// How each tool did over the calls kept, by tool name.
    pub fn tool_latency(&self) -> Vec<ToolLatency> {
        let calls = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
        let mut by_tool: BTreeMap<&str, (Vec<u64>, u64)> = BTreeMap::new();
        for call in calls.iter() {
            let (times, errors) = by_tool.entry(&call.tool).or_default();
            times.push(call.ms);
            if !call.ok {
                *errors += 1;
            }
        }
        by_tool
            .into_iter()
            .map(|(tool, (mut times, errors))| {
                times.sort_unstable();
                ToolLatency {
                    tool: tool.to_string(),
                    calls: times.len() as u64,
                    errors,
                    p50_ms: nearest_rank(&times, 50),
                    p95_ms: nearest_rank(&times, 95),
                }
            })
            .collect()
    }

    /// Writes `run`, replacing the record of the same session (a session ends once, but a record written twice
    /// stays one row).
    pub fn record(&self, run: &Run) -> Result<(), StoreError> {
        let run = run.clone();
        self.db.write(move |conn| {
            let def = def();
            insert_row(conn, def, &run, run.id)
        })?;
        Ok(())
    }

    /// The runs `filter` selects, newest first, one page of `limit` (0: the default page).
    pub fn runs(&self, filter: &RunFilter, limit: usize) -> Vec<Run> {
        let limit = if limit == 0 {
            RUNS_PAGE
        } else {
            limit.min(RUNS_LIMIT)
        };
        let read = self.db.read(|conn| {
            let mut stmt = conn.prepare_cached(sql::EVAL_RUNS_PAGE)?;
            let runs = stmt
                .query_map(
                    params![
                        filter.project,
                        filter.model,
                        filter.seat,
                        bound(limit as u64)
                    ],
                    run_at,
                )?
                .collect::<Result<Vec<Run>, _>>()?;
            Ok(runs)
        });
        read.unwrap_or_else(|e| {
            eprintln!("apid: evals: cannot read the runs: {e}");
            Vec::new()
        })
    }

    /// Every run `filter` selects, oldest first (what the reports fold over).
    pub fn runs_all(&self, filter: &RunFilter) -> Vec<Run> {
        let read = self.db.read(|conn| {
            let mut stmt = conn.prepare_cached(sql::EVAL_RUNS_ALL)?;
            let runs = stmt
                .query_map(params![filter.project, filter.model, filter.seat], run_at)?
                .collect::<Result<Vec<Run>, _>>()?;
            Ok(runs)
        });
        read.unwrap_or_else(|e| {
            eprintln!("apid: evals: cannot read the runs: {e}");
            Vec::new()
        })
    }

    /// How the runs of each step, seat, model and engine did.
    pub fn scores(&self) -> Vec<Score> {
        let read = self.db.read(|conn| {
            let mut stmt = conn.prepare_cached(sql::EVAL_SCORES)?;
            let scores = stmt
                .query_map([], |r| {
                    let runs: u64 = r.get(4)?;
                    let successes: u64 = r.get(5)?;
                    let cost: u64 = r.get(6)?;
                    let wall: u64 = r.get(7)?;
                    let clean: u64 = r.get(8)?;
                    let cost_micros: u64 = r.get(6)?;
                    let wall_ms: u64 = r.get(7)?;
                    Ok(Score {
                        step: r.get(0)?,
                        seat: r.get(1)?,
                        model: r.get(2)?,
                        engine: r.get(3)?,
                        runs,
                        successes,
                        success_rate: successes as f64 / runs.max(1) as f64,
                        cost_per_success_micros: per_success(cost, successes),
                        time_per_success_ms: per_success(wall, successes),
                        quality: clean as f64 / runs.max(1) as f64,
                        cost_micros,
                        wall_ms,
                    })
                })?
                .collect::<Result<Vec<Score>, _>>()?;
            Ok(scores)
        });
        read.unwrap_or_else(|e| {
            eprintln!("apid: evals: cannot read the scores: {e}");
            Vec::new()
        })
    }

    /// The scores of `step` (all steps when None), best first: the highest success rate, then the lower cost per
    /// success, then the more runs.
    pub fn leaderboard(&self, step: Option<&str>) -> Vec<Score> {
        let mut scores: Vec<Score> = self
            .scores()
            .into_iter()
            .filter(|s| step.is_none_or(|step| s.step == step))
            .collect();
        scores.sort_by(|a, b| {
            b.success_rate
                .total_cmp(&a.success_rate)
                .then_with(|| cost_of(a).total_cmp(&cost_of(b)))
                .then_with(|| b.runs.cmp(&a.runs))
        });
        scores
    }

    /// What the runs of `step` on `task` cost so far, in micro-dollars (the step's spend against its budget share).
    pub fn step_spend(&self, project: &str, task: &str, step: &str) -> u64 {
        let filter = RunFilter {
            project: Some(project.to_string()),
            ..RunFilter::default()
        };
        self.runs_all(&filter)
            .iter()
            .filter(|r| r.task.as_deref() == Some(task) && r.step == step)
            .map(|r| r.cost_micros)
            .sum()
    }

    /// The cost report: the runs `filter` selects, grouped by `by`.
    pub fn costs(&self, by: By, filter: &RunFilter) -> Vec<CostRow> {
        report::costs(by, &self.runs_all(filter))
    }

    /// Each reviewer model's precision and misses, from the rulings and the reviewers' passes.
    pub fn reviewers(&self) -> Vec<ReviewerScore> {
        let findings = self.read_findings();
        let passes = self.read_passes();
        let late = self.read_late_tasks();
        report::reviewers(&findings, &passes, &late)
    }

    /// A `run` command a session's VM reported, and whether it passed: a replay's check result (see [`Evals::record_checks`]).
    pub fn ran(&self, cid: u64, command: &str, ok: bool) {
        self.vms()
            .entry(cid)
            .or_default()
            .runs
            .push((command.to_string(), ok));
    }

    /// The task's change landed (or did not): every run of the task says so from now on.
    pub fn landed(&self, task: &str, landed: bool) -> Result<(), StoreError> {
        let task = task.to_string();
        self.db.write(move |conn| {
            conn.prepare_cached(sql::EVAL_RUNS_LANDED)?
                .execute(params![task, landed])?;
            Ok(())
        })?;
        Ok(())
    }

    /// The task's change was reverted: every run of the task says so.
    pub fn reverted(&self, task: &str) -> Result<(), StoreError> {
        let task = task.to_string();
        self.db.write(move |conn| {
            conn.prepare_cached(sql::EVAL_RUNS_REVERTED)?
                .execute(params![task])?;
            Ok(())
        })?;
        Ok(())
    }

    /// An incident traced back to the task's landing: every run of the task counts it.
    pub fn incident(&self, task: &str) -> Result<(), StoreError> {
        let task = task.to_string();
        self.db.write(move |conn| {
            conn.prepare_cached(sql::EVAL_RUNS_INCIDENT)?
                .execute(params![task])?;
            Ok(())
        })?;
        Ok(())
    }

    /// The task passed `stages` qualification stages in the lab, which took `lab_minutes`: every run of the task says so.
    pub fn qualify_run(
        &self,
        run: u64,
        task: &str,
        stages: u64,
        lab_minutes: u64,
    ) -> Result<bool, StoreError> {
        let task = task.to_string();
        let written = self.db.write(move |conn| {
            let seen: u64 = conn
                .prepare_cached(sql::EVAL_QUALIFICATION_SEEN)?
                .query_row(params![bound(run)], |r| r.get(0))?;
            if seen > 0 {
                return Ok(false);
            }
            let row = Qualification {
                id: run,
                task: task.clone(),
                stages,
                lab_minutes,
            };
            insert_row(conn, qualification_def(), &row, run)?;
            conn.prepare_cached(sql::EVAL_RUNS_QUALIFIED)?
                .execute(params![task, bound(stages), bound(lab_minutes)])?;
            Ok(true)
        })?;
        Ok(written.value)
    }

    /// A review ruled on a finding (confirmed or dropped). A confirmed finding is a later finding of the task's builder
    /// runs. Returns the ruling's id.
    pub fn finding(&self, finding: &FindingRecord) -> Result<u64, StoreError> {
        let finding = finding.clone();
        let written = self.db.write(move |conn| {
            let id = next_id(conn, sql::EVAL_FINDING_NEXT)?;
            let mut row = finding;
            row.id = id;
            insert_row(conn, finding_def(), &row, id)?;
            if row.verdict == "confirmed"
                && let Some(task) = &row.task
            {
                conn.prepare_cached(sql::EVAL_RUNS_LATER_FINDING)?
                    .execute(params![task])?;
            }
            Ok(id)
        })?;
        Ok(written.value)
    }

    /// Archives a task with a known outcome as a benchmark case. Returns its id.
    pub fn archive(&self, archived: &Archived) -> Result<u64, StoreError> {
        let archived = archived.clone();
        let written = self.db.write(move |conn| {
            let id = next_id(conn, sql::EVAL_ARCHIVE_NEXT)?;
            let mut row = archived;
            row.id = id;
            insert_row(conn, archive_def(), &row, id)?;
            Ok(id)
        })?;
        Ok(written.value)
    }

    /// The archived tasks, oldest first.
    pub fn archives(&self) -> Vec<Archived> {
        let read = self.db.read(|conn| {
            let mut stmt = conn.prepare_cached(sql::EVAL_ARCHIVES_ALL)?;
            let rows = stmt
                .query_map([], archive_at)?
                .collect::<Result<Vec<Archived>, _>>()?;
            Ok(rows)
        });
        read.unwrap_or_else(|e| {
            eprintln!("apid: evals: cannot read the archive: {e}");
            Vec::new()
        })
    }

    /// Starts the replay of archived case `case` in `task` (the replay task the route opened), on the Weft branch
    /// `reference` that starts at the case's commit. Its record is written as running until the task ends. Returns its id.
    pub fn start_replay(
        &self,
        case: &Archived,
        task: &str,
        reference: &str,
        now: u64,
    ) -> Result<u64, StoreError> {
        let row = BenchRun {
            id: 0,
            archive: case.id,
            task: Some(task.to_string()),
            reference: reference.to_string(),
            command: case.command.clone(),
            check: None,
            outcome: "running".into(),
            known: case.outcome.clone(),
            matched: false,
            cost_micros: 0,
            wall_ms: 0,
            replayed_ms: now,
        };
        let written = self.db.write(move |conn| {
            let id = next_id(conn, sql::EVAL_BENCH_NEXT)?;
            let mut row = row;
            row.id = id;
            insert_row(conn, bench_def(), &row, id)?;
            Ok(id)
        })?;
        Ok(written.value)
    }

    /// The Weft branch of the running replay of `task`, when `task` is one.
    pub fn replay_reference(&self, task: &str) -> Option<String> {
        let task = task.to_string();
        let read = self.db.read(|conn| {
            let reference: Option<String> = conn
                .prepare_cached(sql::EVAL_BENCH_REFERENCE)?
                .query_row(params![task], |r| r.get(0))
                .optional()?;
            Ok(reference)
        });
        read.unwrap_or_else(|e| {
            eprintln!("apid: evals: cannot read the replay branch: {e}");
            None
        })
    }

    /// The `run` commands a replay's sessions reported, in order: the running replay of `task` keeps the last run of its
    /// test command, and its check is that run's result.
    pub fn record_checks(&self, task: &str, runs: &[(String, bool)]) -> Result<(), StoreError> {
        if runs.is_empty() {
            return Ok(());
        }
        let task = task.to_string();
        let runs = runs.to_vec();
        self.db.write(move |conn| {
            for (command, ok) in &runs {
                conn.prepare_cached(sql::EVAL_BENCH_CHECK)?
                    .execute(params![task, command, *ok])?;
            }
            Ok(())
        })?;
        Ok(())
    }

    /// The replay task `task` ended at `now`: `landed` (its change landed) or failed. A running replay of it is scored
    /// against its case's known outcome: it passes when its change landed and, where the case names a test command, that
    /// command's last run passed. A case that landed must pass; a reverted one must not. The record keeps what its
    /// sessions spent and the time it took. A task that is no replay is left alone.
    pub fn replay_finished(&self, task: &str, landed: bool, now: u64) -> Result<(), StoreError> {
        let task = task.to_string();
        self.db.write(move |conn| {
            let running: Option<(u64, String, u64, String, Option<bool>)> = conn
                .prepare_cached(sql::EVAL_BENCH_RUNNING)?
                .query_row(params![task], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
                })
                .optional()?;
            let Some((id, known, started, command, check)) = running else {
                return Ok(());
            };
            let cost: u64 = conn
                .prepare_cached(sql::EVAL_TASK_COST)?
                .query_row(params![task], |r| r.get(0))?;
            let passed = landed && (command.is_empty() || check == Some(true));
            let matched = passed == (known == "landed");
            conn.prepare_cached(sql::EVAL_BENCH_FINISH)?
                .execute(params![
                    id,
                    if passed { "passed" } else { "failed" },
                    matched,
                    bound(cost),
                    bound(now.saturating_sub(started))
                ])?;
            Ok(())
        })?;
        Ok(())
    }

    /// The benchmark's replays, newest first.
    pub fn bench_runs(&self) -> Vec<BenchRun> {
        let read = self.db.read(|conn| {
            let mut stmt = conn.prepare_cached(sql::EVAL_BENCH_ALL)?;
            let rows = stmt
                .query_map([], bench_at)?
                .collect::<Result<Vec<BenchRun>, _>>()?;
            Ok(rows)
        });
        let mut rows = read.unwrap_or_else(|e| {
            eprintln!("apid: evals: cannot read the benchmark: {e}");
            Vec::new()
        });
        rows.reverse();
        rows
    }

    fn read_findings(&self) -> Vec<FindingRecord> {
        let read = self.db.read(|conn| {
            let mut stmt = conn.prepare_cached(sql::EVAL_FINDINGS_ALL)?;
            let rows = stmt
                .query_map([], finding_at)?
                .collect::<Result<Vec<FindingRecord>, _>>()?;
            Ok(rows)
        });
        read.unwrap_or_else(|e| {
            eprintln!("apid: evals: cannot read the findings: {e}");
            Vec::new()
        })
    }

    fn read_passes(&self) -> Vec<Pass> {
        let read = self.db.read(|conn| {
            let mut stmt = conn.prepare_cached(sql::EVAL_PASSES)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(Pass {
                        model: r.get(0)?,
                        task: r.get(1)?,
                        ended_ms: r.get(2)?,
                    })
                })?
                .collect::<Result<Vec<Pass>, _>>()?;
            Ok(rows)
        });
        read.unwrap_or_else(|e| {
            eprintln!("apid: evals: cannot read the passes: {e}");
            Vec::new()
        })
    }

    fn read_late_tasks(&self) -> HashSet<String> {
        let read = self.db.read(|conn| {
            let mut stmt = conn.prepare_cached(sql::EVAL_LATE_TASKS)?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<HashSet<String>, _>>()?;
            Ok(rows)
        });
        read.unwrap_or_else(|e| {
            eprintln!("apid: evals: cannot read the late tasks: {e}");
            HashSet::new()
        })
    }

    /// The record of `session` ends with what its VM did: the in-flight entries are dropped.
    fn take(&self, session: &Session) -> (Begun, Vm) {
        let begun = self.begun().remove(&session.id).unwrap_or_else(|| Begun {
            recipe: "change".to_string(),
            version: CHANGE_VERSION,
            step: session.role.clone(),
            round: session.attempt,
            ..Begun::default()
        });
        let vm = session
            .cid
            .and_then(|cid| self.vms().remove(&cid))
            .unwrap_or_default();
        (begun, vm)
    }
}

/// Writes the eval record of a step session that has just ended. A long-lived session leaves none.
pub fn ended(app: &App, session: &Session) {
    if session.kind != Kind::Step {
        return;
    }
    let (begun, vm) = app.evals.take(session);
    if let Some(task) = &session.task
        && let Err(err) = app.evals.record_checks(task, &vm.runs)
    {
        eprintln!("apid: evals: the checks of {task} were not recorded: {err}");
    }
    let mut run = Run::of(session, begun, vm);
    run.pool = session
        .node
        .as_deref()
        .and_then(|node| crate::placement::known_pool(app, node))
        .map(|pool| pool.key());
    if let Err(err) = app.evals.record(&run) {
        eprintln!(
            "apid: evals: the record of {} was not written: {err}",
            session.id
        );
    }
}

impl Run {
    /// The record of `session`, which has ended: its usage, its model and node, its gate's result, and what its seat
    /// and VM said while it ran.
    fn of(session: &Session, begun: Begun, vm: Vm) -> Run {
        let model = session.model.clone();
        let provider = model
            .as_deref()
            .and_then(|m| m.split_once('/'))
            .map(|(provider, _)| provider.to_string());
        let started = session.started_ms.unwrap_or(session.created_ms);
        let ended_ms = session.ended_ms.unwrap_or(started);
        let ok = session.state == crate::sessions::State::Ended && session.verdict != Some(false);
        let gate = match session.verdict {
            Some(true) => "passed",
            Some(false) => "failed",
            None if ok => "none",
            None => "failed",
        };
        Run {
            id: seq_of(&session.id),
            session: session.id.clone(),
            project: session.project.clone(),
            task: session.task.clone(),
            recipe: begun.recipe,
            step: begun.step,
            seat: session.seat.clone().unwrap_or_default(),
            round: begun.round,
            engine: session.engine.clone(),
            engine_version: env!("CARGO_PKG_VERSION").to_string(),
            model,
            provider,
            harness: vm.harness,
            node: session.node.clone(),
            pool: None,
            tokens_in: session.usage.tokens_in,
            tokens_out: session.usage.tokens_out,
            tokens_cached: session.usage.tokens_cached,
            cost_micros: session.usage.cost_micros,
            model_calls: session.usage.model_calls,
            tool_calls: session.usage.tool_calls,
            tool_ms: vm.tool_ms,
            wall_ms: ended_ms.saturating_sub(started),
            verdict: session.verdict,
            outcome: session.outcome.clone().unwrap_or_default(),
            ok,
            landed: None,
            reverted: None,
            ended_ms,
            recipe_version: begun.version,
            harness_version: vm.harness_version,
            gate: gate.to_string(),
            later_findings: 0,
            incidents: 0,
            memory_cites: vm.memory_cites,
            qual_stages: 0,
            lab_minutes: 0,
            choice: begun.choice,
            choice_reason: begun.choice_reason,
        }
    }
}

/// A session's number: `s12` is 12.
fn seq_of(id: &str) -> u64 {
    id.strip_prefix('s')
        .and_then(|seq| seq.parse().ok())
        .unwrap_or(0)
}

/// The value at percentile `pct` of the sorted `times` (nearest rank; 0 for none).
fn nearest_rank(times: &[u64], pct: usize) -> u64 {
    if times.is_empty() {
        return 0;
    }
    let rank = (times.len() * pct).div_ceil(100).clamp(1, times.len());
    times[rank - 1]
}

/// Micro-dollars or milliseconds per success, or None when nothing succeeded.
fn per_success(total: u64, successes: u64) -> Option<f64> {
    (successes > 0).then(|| total as f64 / successes as f64)
}

/// A score's cost per success, for comparing: a model that never succeeded costs the most.
fn cost_of(score: &Score) -> f64 {
    score.cost_per_success_micros.unwrap_or(f64::INFINITY)
}

/// The runs' table (`eval_run`), written through.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(Run)
}

/// The rulings' table (`eval_finding`), written through.
pub fn finding_def() -> &'static TableDef {
    apz_store::root_def!(FindingRecord)
}

/// The archive's table (`eval_archive`), written through.
pub fn archive_def() -> &'static TableDef {
    apz_store::root_def!(Archived)
}

/// The benchmark's table (`eval_bench`), written through.
pub fn bench_def() -> &'static TableDef {
    apz_store::root_def!(BenchRun)
}

/// A count or an id as a SQL parameter (values past i64 are clamped; no row holds them).
fn bound(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// The next free id of a counter table, by its query.
fn next_id(conn: &Connection, query: &str) -> Result<u64, StoreError> {
    let next: u64 = conn.prepare_cached(query)?.query_row([], |r| r.get(0))?;
    Ok(next)
}

/// Writes one row of `def` through the table's own encoding, keyed by `pos` (a stream has no list position of its
/// own: the id stands in for it).
fn insert_row<T: Table>(
    conn: &Connection,
    def: &TableDef,
    row: &T,
    pos: u64,
) -> Result<(), StoreError> {
    let mut rows = RowSet::new();
    row.put(def, vec![Value::Integer(bound(pos))], &mut rows)?;
    let mut changes = apz_store::table::Changes::default();
    changes.upserts = rows
        .tables
        .remove(&def.name)
        .unwrap_or_default()
        .into_iter()
        .map(|row| (def.name.clone(), row))
        .collect();
    apz_store::table::apply(conn, def, &changes)
}

fn run_at(r: &Row<'_>) -> apz_store::rusqlite::Result<Run> {
    Ok(Run {
        id: r.get(0)?,
        session: r.get(1)?,
        project: r.get(2)?,
        task: r.get(3)?,
        recipe: r.get(4)?,
        step: r.get(5)?,
        seat: r.get(6)?,
        round: r.get(7)?,
        engine: r.get(8)?,
        engine_version: r.get(9)?,
        model: r.get(10)?,
        provider: r.get(11)?,
        harness: r.get(12)?,
        node: r.get(13)?,
        pool: r.get(14)?,
        tokens_in: r.get(15)?,
        tokens_out: r.get(16)?,
        tokens_cached: r.get(17)?,
        cost_micros: r.get(18)?,
        model_calls: r.get(19)?,
        tool_calls: r.get(20)?,
        tool_ms: r.get(21)?,
        wall_ms: r.get(22)?,
        verdict: r.get(23)?,
        outcome: r.get(24)?,
        ok: r.get(25)?,
        landed: r.get(26)?,
        reverted: r.get(27)?,
        ended_ms: r.get(28)?,
        recipe_version: r.get(29)?,
        harness_version: r.get(30)?,
        gate: r.get(31)?,
        later_findings: r.get(32)?,
        incidents: r.get(33)?,
        memory_cites: r.get(34)?,
        qual_stages: r.get(35)?,
        lab_minutes: r.get(36)?,
        choice: r.get(37)?,
        choice_reason: r.get(38)?,
    })
}

fn finding_at(r: &Row<'_>) -> apz_store::rusqlite::Result<FindingRecord> {
    Ok(FindingRecord {
        id: r.get(0)?,
        project: r.get(1)?,
        task: r.get(2)?,
        review: r.get(3)?,
        finding: r.get(4)?,
        reviewer: r.get(5)?,
        reviewer_seat: r.get(6)?,
        verifier: r.get(7)?,
        verdict: r.get(8)?,
        severity: r.get(9)?,
        ruled_ms: r.get(10)?,
    })
}

fn archive_at(r: &Row<'_>) -> apz_store::rusqlite::Result<Archived> {
    Ok(Archived {
        id: r.get(0)?,
        project: r.get(1)?,
        task: r.get(2)?,
        start: r.get(3)?,
        goal: r.get(4)?,
        check: r.get(5)?,
        command: r.get(6)?,
        outcome: r.get(7)?,
        archived_ms: r.get(8)?,
    })
}

fn bench_at(r: &Row<'_>) -> apz_store::rusqlite::Result<BenchRun> {
    Ok(BenchRun {
        id: r.get(0)?,
        archive: r.get(1)?,
        task: r.get(2)?,
        reference: r.get(3)?,
        command: r.get(4)?,
        check: r.get(5)?,
        outcome: r.get(6)?,
        known: r.get(7)?,
        matched: r.get(8)?,
        cost_micros: r.get(9)?,
        wall_ms: r.get(10)?,
        replayed_ms: r.get(11)?,
    })
}

/// A cost report's query: the grouping (`model` when none is named) and the run filter. An unknown grouping is refused.
pub fn cost_query(by: Option<&str>, filter: RunFilter) -> Result<(By, RunFilter), String> {
    let name = by.unwrap_or("model");
    By::parse(name)
        .map(|by| (by, filter))
        .ok_or_else(|| format!("by must be project, task, step or model, not {name:?}"))
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/evals/runs", |app, req, _| runs_route(app, req))
        .doc(RouteDoc::new("List the eval records of step sessions, newest first").errors(&[400]))
        .get("/v1/evals/scores", |app, _, _| {
            Response::json(200, &app.evals.scores())
        })
        .doc(RouteDoc::new(
            "Score the step sessions by step, seat, model and engine",
        ))
        .get("/v1/evals/leaderboard", |app, req, _| {
            Response::json(200, &app.evals.leaderboard(text_param(req, "step").as_deref()))
        })
        .doc(RouteDoc::new(
            "Rank the scores of a step (or every step) by success rate, then cost per success",
        ))
        .get("/v1/evals/costs", |app, req, _| costs_route(app, req))
        .doc(RouteDoc::new(
            "Report what the step sessions cost and how they did, grouped by project, task, step or model",
        )
        .errors(&[400]))
        .get("/v1/evals/reviewers", |app, _, _| {
            Response::json(200, &app.evals.reviewers())
        })
        .doc(RouteDoc::new(
            "Score each reviewer model by its precision (confirmed findings over its findings) and its misses",
        ))
        .get("/v1/evals/archive", |app, _, _| {
            Response::json(200, &app.evals.archives())
        })
        .doc(RouteDoc::new("List the archived tasks with a known outcome, the benchmark's cases"))
        .get("/v1/evals/bench", |app, _, _| {
            Response::json(200, &app.evals.bench_runs())
        })
        .doc(RouteDoc::new("List the benchmark's replays of archived tasks, newest first"))
        .post("/v1/evals/archive", |app, req: &Request, _| archive_route(app, req))
        .doc(
            RouteDoc::new("Archive a task with its known outcome as a benchmark case")
                .status(201)
                .errors(&[400]),
        )
        .post("/v1/evals/qualified", |app, req: &Request, _| qualified_route(app, req))
        .doc(RouteDoc::new("Record the qualification stages a task passed and the lab minutes they took").errors(&[400]))
        .post("/v1/evals/bench/{id}", |app, _, p| replay_route(app, p.get("id")))
        .doc(
            RouteDoc::new(
                "Replay archived case {id}: a task in its project at the commit it started from, scored against the case's outcome when it ends",
            )
            .status(201)
            .errors(&[400, 404, 409]),
        )
        .get("/v1/evals/tools", |app, _, _| {
            Response::json(200, &app.evals.tool_latency())
        })
        .doc(RouteDoc::new(
            "Show each tool's calls, errors and latency (p50, p95)",
        ))
}

/// A query parameter, percent-decoded; an empty one is absent.
fn text_param(req: &Request, name: &str) -> Option<String> {
    req.query_param(name)
        .map(crate::plaza::percent_decode)
        .filter(|v| !v.is_empty())
}

fn runs_route(app: &App, req: &Request) -> Response {
    let limit = match req.query_param("limit") {
        None => 0,
        Some(raw) => match raw.parse::<usize>() {
            Ok(n) if (1..=RUNS_LIMIT).contains(&n) => n,
            _ => {
                return Response::error(
                    400,
                    &format!("limit must be 1..={RUNS_LIMIT}, not {raw:?}"),
                );
            }
        },
    };
    let filter = RunFilter {
        project: text_param(req, "project"),
        model: text_param(req, "model"),
        seat: text_param(req, "seat"),
    };
    Response::json(200, &app.evals.runs(&filter, limit))
}

/// Archives a case: a body naming the project, the task, the commit it started from, its goal, its check and its outcome
/// (`landed` or `reverted`).
fn archive_route(app: &App, req: &Request) -> Response {
    let body: serde_json::Value = match serde_json::from_slice(&req.body) {
        Ok(body) => body,
        Err(err) => return Response::error(400, &format!("the body is not JSON: {err}")),
    };
    let text = |key: &str| {
        body.get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    let (Some(project), Some(task), Some(start), Some(goal), Some(check), Some(outcome)) = (
        text("project"),
        text("task"),
        text("start"),
        text("goal"),
        text("check"),
        text("outcome"),
    ) else {
        return Response::error(
            400,
            "a case names its project, task, start, goal, check and outcome",
        );
    };
    if !matches!(outcome.as_str(), "landed" | "reverted") {
        return Response::error(
            400,
            &format!("outcome is landed or reverted, not {outcome:?}"),
        );
    }
    let case = Archived {
        id: 0,
        project,
        task,
        start,
        goal,
        check,
        command: text("command").unwrap_or_default(),
        outcome,
        archived_ms: apz_plaza::now_ms(),
    };
    match app.evals.archive(&case) {
        Ok(id) => Response::json(201, &json!({ "id": id })),
        Err(err) => Response::error(500, &format!("the case was not archived: {err}")),
    }
}

/// Opens the replay of archived case `id`. First a Weft branch is made at the case's commit (`ref.create`); then the
/// replay's task is created outside any project, its replay is recorded as running on that branch, and only then is the
/// task linked into the case's project. The work loop therefore never sees the task without its branch, so the replay's
/// lane starts at the case's commit and lands only to that branch, never to main. Its end scores it
/// ([`Evals::replay_finished`]); the branch stays as evidence.
fn replay_route(app: &App, id: &str) -> Response {
    let Ok(id) = id.parse::<u64>() else {
        return Response::error(400, &format!("{id:?} is no case number"));
    };
    let Some(case) = app.evals.archives().into_iter().find(|c| c.id == id) else {
        return Response::error(404, &format!("no archived case {id}"));
    };
    let now = apz_plaza::now_ms();
    let repo = match crate::weft_hub::ensure_repo(app, &case.project) {
        Ok(repo) => repo,
        Err(err) => return Response::error(409, &format!("the project has no repository: {err}")),
    };
    let reference = format!("replay-{id}-{now}");
    if let Err(err) = app.weft.console(
        "ref.create",
        json!({ "repo": repo, "ref": reference, "at": case.start }),
    ) {
        return Response::error(
            409,
            &format!("the replay branch was not made at {}: {err}", case.start),
        );
    }
    let task = match app.plaza.tasks.create(&apz_plaza::tasks::CreateTask {
        goal: format!(
            "Replay archived case {id}: {}. Work on your lane, which starts at revision {}, the commit the task started from.",
            case.goal, case.start
        ),
        acceptance: Some(case.check.clone()),
        role: None,
        after: vec![],
        project: None,
    }) {
        Ok(task) => task,
        Err(err) => {
            return Response::error(409, &format!("the replay task was not created: {err:?}"));
        }
    };
    let bench = match app.evals.start_replay(&case, task.id(), &reference, now) {
        Ok(bench) => bench,
        Err(err) => return Response::error(500, &format!("the replay was not recorded: {err}")),
    };
    // Only now does the task join its project: `adopt` moves a task that is in no project into one, and each moved
    // task is linked to it, as the owner's own adoption does.
    let moved = match app.plaza.tasks.adopt(task.id(), &case.project) {
        Ok(moved) => moved,
        Err(refused) => {
            return Response::error(409, &format!("the replay task was not adopted: {refused}"));
        }
    };
    for moved_task in &moved {
        if let Err(err) = app.projects.link_task(&case.project, moved_task.id()) {
            eprintln!(
                "apid: replay task {} not linked to {}: {err}",
                moved_task.id(),
                case.project
            );
        }
    }
    crate::work_loop::tick(app);
    Response::json(
        201,
        &json!({ "task": task.id(), "bench": bench, "reference": reference }),
    )
}

/// Records the qualification stages a lab run passed and the lab minutes it held, on its change's task: keyed by the run, so
/// judging the same run again records nothing new. A body names the run (`q<n>`), the task, the stages and the minutes.
fn qualified_route(app: &App, req: &Request) -> Response {
    let body: serde_json::Value = match serde_json::from_slice(&req.body) {
        Ok(body) => body,
        Err(err) => return Response::error(400, &format!("the body is not JSON: {err}")),
    };
    let run = body.get("run").and_then(|run| match run {
        serde_json::Value::Number(n) => n.as_u64(),
        serde_json::Value::String(s) => s.trim_start_matches('q').parse().ok(),
        _ => None,
    });
    let (Some(run), Some(task), Some(stages), Some(minutes)) = (
        run,
        body.get("task").and_then(serde_json::Value::as_str),
        body.get("stages").and_then(serde_json::Value::as_u64),
        body.get("lab_minutes").and_then(serde_json::Value::as_u64),
    ) else {
        return Response::error(
            400,
            "a qualification names its run, task, stages and lab_minutes",
        );
    };
    match app.evals.qualify_run(run, task, stages, minutes) {
        Ok(recorded) => Response::json(
            200,
            &json!({ "run": run, "task": task, "recorded": recorded }),
        ),
        Err(err) => Response::error(500, &format!("the qualification was not recorded: {err}")),
    }
}

fn costs_route(app: &App, req: &Request) -> Response {
    let filter = RunFilter {
        project: text_param(req, "project"),
        model: text_param(req, "model"),
        seat: text_param(req, "seat"),
    };
    match cost_query(text_param(req, "by").as_deref(), filter) {
        Ok((by, filter)) => Response::json(200, &app.evals.costs(by, &filter)),
        Err(problem) => Response::error(400, &problem),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::{NewSession, Sessions, State, Usage};

    /// Evals over a fresh plaza.db in a temporary folder.
    fn fixture() -> (tempfile::TempDir, apz_store::Store, Evals) {
        let dir = tempfile::tempdir().unwrap();
        let store = apz_store::Store::open(dir.path(), crate::store::FILES).unwrap();
        let evals = Evals::new(Arc::clone(store.db(crate::store::PLAZA)));
        (dir, store, evals)
    }

    /// The record of session `id` of `task`'s `seat`, running `step` on `model`.
    fn run_step(
        id: u64,
        task: &str,
        seat: &str,
        step: &str,
        model: &str,
        ok: bool,
        cost: u64,
        wall: u64,
    ) -> Run {
        Run {
            id,
            session: format!("s{id}"),
            project: "pr1".into(),
            task: Some(task.into()),
            recipe: "change".into(),
            step: step.into(),
            seat: seat.into(),
            round: 1,
            engine: "harness".into(),
            engine_version: "0.1.0".into(),
            model: Some(model.into()),
            provider: Some("mock".into()),
            harness: "acp".into(),
            node: None,
            pool: None,
            tokens_in: 10,
            tokens_out: 5,
            tokens_cached: 0,
            cost_micros: cost,
            model_calls: 1,
            tool_calls: 2,
            tool_ms: 30,
            wall_ms: wall,
            verdict: None,
            outcome: "the step ended".into(),
            ok,
            landed: None,
            reverted: None,
            ended_ms: 1000 + id,
            recipe_version: 2,
            harness_version: String::new(),
            gate: "none".into(),
            later_findings: 0,
            incidents: 0,
            memory_cites: 0,
            qual_stages: 0,
            lab_minutes: 0,
            choice: String::new(),
            choice_reason: String::new(),
        }
    }

    /// The record of session `id` of `task`'s `seat`, run on `model` (its step is `build`).
    fn run(id: u64, task: &str, seat: &str, model: &str, ok: bool, cost: u64, wall: u64) -> Run {
        run_step(id, task, seat, "build", model, ok, cost, wall)
    }

    fn ids(runs: &[Run]) -> Vec<u64> {
        runs.iter().map(|r| r.id).collect()
    }

    #[test]
    fn a_step_with_model_eval_takes_the_model_its_evals_favour_and_takes_another_when_they_change()
    {
        let (_dir, _store, evals) = fixture();
        let policy = choose::Policy {
            min_samples: 5,
            explore_pct: 0,
        };
        let models = vec!["mock/a".to_string(), "mock/b".to_string()];
        let next = |evals: &Evals, run: u64| {
            let scores = evals.scores();
            choose::model(&scores, "build", "harness", &models, None, policy, run).unwrap()
        };
        // A has five runs and all succeed; B has five and two succeed.
        for id in 1..=5 {
            evals
                .record(&run(id, "t1", "builder", "mock/a", true, 100, 900))
                .unwrap();
        }
        for id in 6..=10 {
            evals
                .record(&run(id, "t1", "builder", "mock/b", id <= 7, 100, 900))
                .unwrap();
        }
        let first = next(&evals, 11);
        assert_eq!(
            (first.key.as_str(), first.rule),
            ("mock/a", choose::EXPLOITED)
        );
        assert!(
            first.reason.contains("mock/a succeeds 100% of 5 runs"),
            "{}",
            first.reason
        );
        // Reseeded: B takes twenty more successes and A fails ten runs, so B is better and the next run takes it.
        for id in 11..=30 {
            evals
                .record(&run(id, "t2", "builder", "mock/b", true, 100, 900))
                .unwrap();
        }
        for id in 31..=40 {
            evals
                .record(&run(id, "t2", "builder", "mock/a", false, 100, 900))
                .unwrap();
        }
        let second = next(&evals, 41);
        assert_eq!(second.key, "mock/b");
        assert_eq!(second.rule, choose::EXPLOITED);
        // What the step spent on its task is the sum of its runs' cost.
        assert_eq!(evals.step_spend("pr1", "t2", "build"), 3000);
    }

    #[test]
    fn a_choice_is_stored_with_its_run_and_reads_back() {
        let (_dir, _store, evals) = fixture();
        let mut chosen = run(1, "t1", "builder", "mock/a", true, 100, 900);
        chosen.choice = choose::EXPLORED.into();
        chosen.choice_reason = "run 1 is in the 10% explore share".into();
        evals.record(&chosen).unwrap();
        evals
            .record(&run(2, "t1", "builder", "mock/b", true, 100, 900))
            .unwrap();
        let back = evals.runs(&RunFilter::default(), 0);
        assert_eq!(back[1], chosen, "the choice reads back with the run");
        assert_eq!(back[0].choice, "", "a fixed step's run names no choice");
        let scores = evals.scores();
        assert!(
            scores
                .iter()
                .all(|s| s.cost_micros == 100 && s.wall_ms == 900)
        );
    }

    #[test]
    fn a_record_is_written_once_and_reads_back_whole_and_filtered() {
        let (_dir, _store, evals) = fixture();
        let first = run(1, "t1", "builder", "mock/agent", true, 100, 900);
        evals.record(&first).unwrap();
        evals
            .record(&run(2, "t1", "reviewer", "mock/echo", true, 40, 300))
            .unwrap();
        // Written again, a record is replaced, not added to.
        evals
            .record(&run(2, "t1", "reviewer", "mock/echo", true, 40, 300))
            .unwrap();
        let all = evals.runs(&RunFilter::default(), 0);
        assert_eq!(ids(&all), vec![2, 1], "newest first, one row each");
        assert_eq!(all[1], first, "the record reads back as it was written");
        let by_model = RunFilter {
            model: Some("mock/agent".into()),
            ..RunFilter::default()
        };
        assert_eq!(ids(&evals.runs(&by_model, 0)), vec![1]);
        let by_seat = RunFilter {
            seat: Some("reviewer".into()),
            ..RunFilter::default()
        };
        assert_eq!(ids(&evals.runs(&by_seat, 0)), vec![2]);
        let other_project = RunFilter {
            project: Some("pr2".into()),
            ..RunFilter::default()
        };
        assert!(evals.runs(&other_project, 0).is_empty());
        assert_eq!(
            evals.runs(&RunFilter::default(), 1).len(),
            1,
            "a limit caps the page"
        );
        assert_eq!(
            ids(&evals.runs_all(&RunFilter::default())),
            vec![1, 2],
            "every run, oldest first"
        );
    }

    #[test]
    fn a_landed_change_marks_every_run_of_its_task() {
        let (_dir, _store, evals) = fixture();
        evals
            .record(&run(1, "t1", "builder", "mock/agent", true, 1, 1))
            .unwrap();
        evals
            .record(&run(2, "t1", "reviewer", "mock/echo", true, 1, 1))
            .unwrap();
        evals
            .record(&run(3, "t2", "builder", "mock/agent", true, 1, 1))
            .unwrap();
        evals.landed("t1", true).unwrap();
        let landed: Vec<(u64, Option<bool>)> = evals
            .runs(&RunFilter::default(), 0)
            .iter()
            .map(|r| (r.id, r.landed))
            .collect();
        assert_eq!(landed, vec![(3, None), (2, Some(true)), (1, Some(true))]);
    }

    #[test]
    fn scores_count_the_successes_and_their_cost_and_time_per_step_seat_model_and_engine() {
        let (_dir, _store, evals) = fixture();
        evals
            .record(&run(1, "t1", "builder", "mock/agent", true, 100, 900))
            .unwrap();
        evals
            .record(&run(2, "t2", "builder", "mock/agent", true, 100, 1100))
            .unwrap();
        evals
            .record(&run(3, "t3", "builder", "mock/agent", false, 50, 500))
            .unwrap();
        evals
            .record(&run(4, "t4", "reviewer", "mock/echo", false, 10, 100))
            .unwrap();
        let scores = evals.scores();
        assert_eq!(scores.len(), 2, "{scores:?}");
        let builder = &scores[0];
        assert_eq!(builder.seat, "builder");
        assert_eq!(builder.step, "build");
        assert_eq!((builder.runs, builder.successes), (3, 2));
        assert!((builder.success_rate - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(builder.cost_per_success_micros, Some(125.0));
        assert_eq!(builder.time_per_success_ms, Some(1250.0));
        assert!(
            (builder.quality - 2.0 / 3.0).abs() < 1e-9,
            "both successes are clean"
        );
        let reviewer = &scores[1];
        assert_eq!((reviewer.runs, reviewer.successes), (1, 0));
        assert_eq!(
            (reviewer.success_rate, reviewer.cost_per_success_micros),
            (0.0, None),
            "no success, no cost per success"
        );
    }

    #[test]
    fn costs_and_the_leaderboard_report_cost_and_success_per_step_and_model_over_seeded_runs() {
        let (_dir, _store, evals) = fixture();
        // Build runs on two models, one review run; the cheap model succeeds less.
        evals
            .record(&run_step(
                1,
                "t1",
                "builder",
                "build",
                "mock/agent",
                true,
                300,
                1000,
            ))
            .unwrap();
        evals
            .record(&run_step(
                2,
                "t2",
                "builder",
                "build",
                "mock/agent",
                true,
                300,
                3000,
            ))
            .unwrap();
        evals
            .record(&run_step(
                3,
                "t3",
                "builder",
                "build",
                "mock/cheap",
                false,
                40,
                800,
            ))
            .unwrap();
        evals
            .record(&run_step(
                4,
                "t3",
                "reviewer",
                "review",
                "mock/echo",
                true,
                20,
                200,
            ))
            .unwrap();

        let by_step = evals
            .costs(By::Step, &RunFilter::default())
            .into_iter()
            .map(|r| (r.key.clone(), r))
            .collect::<BTreeMap<_, _>>();
        let build = &by_step["build"];
        assert_eq!((build.runs, build.successes), (3, 2));
        assert_eq!(build.cost_micros, 640);
        assert_eq!(build.cost_per_success_micros, Some(320.0));
        assert_eq!(by_step["review"].successes, 1);

        let by_model = evals.costs(By::Model, &RunFilter::default());
        let agent = by_model.iter().find(|r| r.key == "mock/agent").unwrap();
        assert_eq!((agent.runs, agent.successes), (2, 2));
        assert_eq!(agent.time_per_success_ms, Some(2000.0));
        let cheap = by_model.iter().find(|r| r.key == "mock/cheap").unwrap();
        assert_eq!((cheap.success_rate, cheap.cost_micros), (0.0, 40));

        let build_only = RunFilter {
            seat: Some("builder".into()),
            ..RunFilter::default()
        };
        assert_eq!(evals.costs(By::Project, &build_only)[0].runs, 3);

        let board = evals.leaderboard(Some("build"));
        assert_eq!(board.len(), 2, "{board:?}");
        assert_eq!(
            board[0].model.as_deref(),
            Some("mock/agent"),
            "the best success rate first"
        );
        assert_eq!(board[1].model.as_deref(), Some("mock/cheap"));
        assert_eq!(evals.leaderboard(Some("nope")).len(), 0);

        assert_eq!(
            cost_query(Some("step"), RunFilter::default()).unwrap().0,
            By::Step
        );
        assert_eq!(cost_query(None, RunFilter::default()).unwrap().0, By::Model);
        assert!(cost_query(Some("seat"), RunFilter::default()).is_err());
    }

    #[test]
    fn later_outcomes_update_the_runs_of_their_task_and_a_confirmed_finding_counts_against_the_builder()
     {
        let (_dir, _store, evals) = fixture();
        evals
            .record(&run_step(
                1,
                "t1",
                "builder",
                "build",
                "mock/agent",
                true,
                100,
                900,
            ))
            .unwrap();
        evals
            .record(&run_step(
                2,
                "t1",
                "reviewer",
                "review",
                "mock/echo",
                true,
                40,
                300,
            ))
            .unwrap();
        evals
            .record(&run_step(
                3,
                "t2",
                "builder",
                "build",
                "mock/agent",
                true,
                100,
                900,
            ))
            .unwrap();
        let finding = |verdict: &str| FindingRecord {
            id: 0,
            project: "pr1".into(),
            task: Some("t1".into()),
            review: "r1".into(),
            finding: "f1".into(),
            reviewer: "mock/echo".into(),
            reviewer_seat: "reviewer".into(),
            verifier: "mock/third".into(),
            verdict: verdict.into(),
            severity: "major".into(),
            ruled_ms: 5000,
        };
        assert_eq!(evals.finding(&finding("confirmed")).unwrap(), 1);
        assert_eq!(evals.finding(&finding("dropped")).unwrap(), 2);
        evals
            .record(&run_step(
                4,
                "t3",
                "builder",
                "build",
                "mock/agent",
                true,
                100,
                900,
            ))
            .unwrap();
        evals.incident("t1").unwrap();
        evals.reverted("t3").unwrap();
        evals.qualify_run(1, "t1", 2, 45).unwrap();

        let runs = evals.runs_all(&RunFilter::default());
        let by_id = |id: u64| runs.iter().find(|r| r.id == id).unwrap();
        assert_eq!(
            by_id(1).later_findings,
            1,
            "the builder carries the confirmed finding"
        );
        assert_eq!(by_id(2).later_findings, 0, "a reviewer does not");
        assert_eq!(by_id(1).incidents, 1);
        assert_eq!(
            by_id(2).incidents,
            1,
            "the incident counts on every run of the task"
        );
        assert_eq!((by_id(1).qual_stages, by_id(1).lab_minutes), (2, 45));
        assert_eq!(by_id(4).reverted, Some(true), "t3's build was reverted");
        assert_eq!(by_id(1).reverted, None, "t1 was not reverted");
        // Only the build of t2 stayed clean: t1 has a later finding and an incident, t3 was reverted.
        let build = evals
            .scores()
            .into_iter()
            .find(|s| s.seat == "builder" && s.model.as_deref() == Some("mock/agent"))
            .unwrap();
        assert_eq!((build.runs, build.successes), (3, 3));
        assert!((build.quality - 1.0 / 3.0).abs() < 1e-9, "{build:?}");
    }

    #[test]
    fn reviewers_are_scored_on_their_rulings_and_on_what_they_missed_after_passing() {
        let (_dir, _store, evals) = fixture();
        // Y reviewed t1 and passed it at 400 (its session ended then); X's confirmed finding on t1 was ruled at 500.
        let mut pass = run_step(1, "t1", "reviewer", "review", "mock/y", true, 10, 100);
        pass.verdict = Some(true);
        pass.ended_ms = 400;
        evals.record(&pass).unwrap();
        let ruling = |reviewer: &str, verdict: &str, ruled_ms: u64| FindingRecord {
            id: 0,
            project: "pr1".into(),
            task: Some("t1".into()),
            review: "r1".into(),
            finding: "f".into(),
            reviewer: reviewer.into(),
            reviewer_seat: "reviewer".into(),
            verifier: "mock/third".into(),
            verdict: verdict.into(),
            severity: "major".into(),
            ruled_ms,
        };
        evals.finding(&ruling("mock/x", "confirmed", 500)).unwrap();
        evals.finding(&ruling("mock/y", "dropped", 300)).unwrap();
        let scores = evals.reviewers();
        let x = scores.iter().find(|s| s.model == "mock/x").unwrap();
        let y = scores.iter().find(|s| s.model == "mock/y").unwrap();
        assert_eq!((x.findings, x.confirmed, x.precision), (1, 1, Some(1.0)));
        assert_eq!((y.findings, y.dropped, y.precision), (1, 1, Some(0.0)));
        assert_eq!(y.changes_passed, 1);
        assert_eq!(
            y.missed_findings, 1,
            "X's confirmed finding came after Y passed t1"
        );
        assert_eq!(y.misses, 1);
    }

    #[test]
    fn a_replay_is_scored_against_its_cases_known_outcome_when_its_task_ends() {
        let (_dir, _store, evals) = fixture();
        let landed = Archived {
            id: 0,
            project: "pr1".into(),
            task: "t1".into(),
            start: "abc123".into(),
            goal: "fix the queue".into(),
            check: "the queue is tested".into(),
            command: String::new(),
            outcome: "landed".into(),
            archived_ms: 42,
        };
        let landed_id = evals.archive(&landed).unwrap();
        assert_eq!(landed_id, 1);
        let landed = Archived {
            id: landed_id,
            ..landed
        };
        // The replay's sessions are its task's runs: the builder and the reviewer.
        evals
            .record(&run_step(
                1,
                "r1",
                "builder",
                "build",
                "mock/agent",
                true,
                250,
                900,
            ))
            .unwrap();
        evals
            .record(&run_step(
                2,
                "r1",
                "reviewer",
                "review",
                "mock/echo",
                true,
                40,
                300,
            ))
            .unwrap();
        assert_eq!(
            evals
                .start_replay(&landed, "r1", "replay-1-900", 1000)
                .unwrap(),
            1
        );
        assert_eq!(
            evals.bench_runs()[0].outcome,
            "running",
            "a replay is recorded while its task runs"
        );
        assert_eq!(
            evals.replay_reference("r1").as_deref(),
            Some("replay-1-900")
        );
        assert_eq!(
            evals.replay_reference("t9"),
            None,
            "a task that is no replay has no branch"
        );
        evals.replay_finished("r1", true, 3000).unwrap();
        let replay = evals
            .bench_runs()
            .into_iter()
            .find(|b| b.archive == landed_id)
            .unwrap();
        assert_eq!(
            (
                replay.task.as_deref(),
                replay.outcome.as_str(),
                replay.known.as_str(),
                replay.matched
            ),
            (Some("r1"), "passed", "landed", true),
            "a case that landed passes when its replay lands"
        );
        assert_eq!(
            (replay.cost_micros, replay.wall_ms),
            (290, 2000),
            "the task's spend and its time"
        );

        // A failed replay of a landed case is a mismatch.
        let failing_id = evals.archive(&landed).unwrap();
        evals
            .start_replay(
                &Archived {
                    id: failing_id,
                    ..landed.clone()
                },
                "r3",
                "replay-2-900",
                10,
            )
            .unwrap();
        evals.replay_finished("r3", false, 20).unwrap();
        let replay = evals
            .bench_runs()
            .into_iter()
            .find(|b| b.archive == failing_id)
            .unwrap();
        assert_eq!(
            (replay.outcome.as_str(), replay.matched),
            ("failed", false),
            "a failed replay of a landed case"
        );

        // A reverted case that fails its replay agrees with its outcome.
        let reverted = Archived {
            id: 0,
            outcome: "reverted".into(),
            ..landed.clone()
        };
        let reverted_id = evals.archive(&reverted).unwrap();
        evals
            .start_replay(
                &Archived {
                    id: reverted_id,
                    ..reverted
                },
                "r2",
                "replay-3-900",
                10,
            )
            .unwrap();
        evals.replay_finished("r2", false, 20).unwrap();
        let replay = evals
            .bench_runs()
            .into_iter()
            .find(|b| b.archive == reverted_id)
            .unwrap();
        assert_eq!(
            (replay.outcome.as_str(), replay.matched, replay.cost_micros),
            ("failed", true, 0)
        );

        // A task that is no replay is left alone.
        evals.replay_finished("t9", true, 5).unwrap();
        assert_eq!(evals.bench_runs().len(), 3);
    }

    #[test]
    fn a_case_with_a_test_command_passes_only_when_its_last_run_of_that_command_passed() {
        let (_dir, _store, evals) = fixture();
        let case = Archived {
            id: 0,
            project: "pr1".into(),
            task: "t1".into(),
            start: "abc123".into(),
            goal: "write the notes".into(),
            check: "the notes exist".into(),
            command: "test -d notes".into(),
            outcome: "landed".into(),
            archived_ms: 0,
        };
        let id = evals.archive(&case).unwrap();
        let case = Archived { id, ..case };
        // The change lands, and the builder ran the command and it passed.
        evals.start_replay(&case, "c1", "replay-a", 0).unwrap();
        evals
            .record_checks(
                "c1",
                &[("ls".into(), false), ("test -d notes".into(), true)],
            )
            .unwrap();
        evals.replay_finished("c1", true, 10).unwrap();
        let passed = evals
            .bench_runs()
            .into_iter()
            .find(|b| b.task.as_deref() == Some("c1"))
            .unwrap();
        assert_eq!(
            (passed.outcome.as_str(), passed.matched, passed.check),
            ("passed", true, Some(true))
        );

        // The change lands, but the command never ran in the replay: it does not pass.
        evals.start_replay(&case, "c2", "replay-b", 0).unwrap();
        evals.replay_finished("c2", true, 10).unwrap();
        let unchecked = evals
            .bench_runs()
            .into_iter()
            .find(|b| b.task.as_deref() == Some("c2"))
            .unwrap();
        assert_eq!(
            (unchecked.outcome.as_str(), unchecked.matched),
            ("failed", false),
            "a check that did not run is no pass"
        );

        // The last run of the command failed: a landed change does not pass.
        evals.start_replay(&case, "c3", "replay-c", 0).unwrap();
        evals
            .record_checks(
                "c3",
                &[
                    ("test -d notes".into(), true),
                    ("test -d notes".into(), false),
                ],
            )
            .unwrap();
        evals.replay_finished("c3", true, 10).unwrap();
        let failed = evals
            .bench_runs()
            .into_iter()
            .find(|b| b.task.as_deref() == Some("c3"))
            .unwrap();
        assert_eq!(
            (failed.outcome.as_str(), failed.matched, failed.check),
            ("failed", false, Some(false))
        );
    }

    #[test]
    fn a_lab_run_is_qualified_once_however_often_it_is_judged() {
        let (_dir, _store, evals) = fixture();
        evals
            .record(&run_step(
                1,
                "t1",
                "builder",
                "build",
                "mock/agent",
                true,
                100,
                900,
            ))
            .unwrap();
        assert!(
            evals.qualify_run(7, "t1", 2, 45).unwrap(),
            "the first judgement is recorded"
        );
        assert!(
            !evals.qualify_run(7, "t1", 3, 60).unwrap(),
            "the same run judged again records nothing"
        );
        let run = evals.runs(&RunFilter::default(), 0).remove(0);
        assert_eq!(
            (run.qual_stages, run.lab_minutes),
            (2, 45),
            "the first judgement counts, once"
        );
        assert!(
            evals.qualify_run(8, "t1", 1, 5).unwrap(),
            "another run is recorded"
        );
        let run = evals.runs(&RunFilter::default(), 0).remove(0);
        assert_eq!(
            (run.qual_stages, run.lab_minutes),
            (3, 50),
            "the two runs add up"
        );
    }

    #[test]
    fn tool_latency_is_the_nearest_rank_per_tool_over_the_calls_kept() {
        let (_dir, _store, evals) = fixture();
        for ms in [10, 40, 20, 30] {
            evals.tool(1, "read", ms, true);
        }
        evals.tool(2, "run", 500, false);
        assert_eq!(
            evals.tool_latency(),
            vec![
                ToolLatency {
                    tool: "read".into(),
                    calls: 4,
                    errors: 0,
                    p50_ms: 20,
                    p95_ms: 40,
                },
                ToolLatency {
                    tool: "run".into(),
                    calls: 1,
                    errors: 1,
                    p50_ms: 500,
                    p95_ms: 500,
                },
            ]
        );
        assert_eq!(nearest_rank(&[], 50), 0);
        assert_eq!(nearest_rank(&[7], 95), 7);
    }

    #[test]
    fn the_hub_keeps_only_the_newest_tool_calls() {
        let (_dir, _store, evals) = fixture();
        evals.tool(1, "old", 1, true);
        for _ in 0..TOOL_CALLS_KEPT {
            evals.tool(1, "new", 2, true);
        }
        let latency = evals.tool_latency();
        assert_eq!(latency.len(), 1, "{latency:?}");
        assert_eq!(
            (latency[0].tool.as_str(), latency[0].calls),
            ("new", TOOL_CALLS_KEPT as u64)
        );
    }

    #[test]
    fn a_session_record_takes_its_usage_model_tool_time_and_memory_reads_from_the_session_and_its_vm()
     {
        let sessions = Sessions::default();
        sessions.create(
            NewSession {
                project: "pr1".into(),
                task: Some("t1".into()),
                seat: Some("builder".into()),
                kind: crate::sessions::Kind::Step,
                parent: None,
                role: "builder".into(),
                engine: "harness".into(),
                model: Some("mock/agent".into()),
                attempt: 2,
            },
            100,
        );
        for to in [State::Placing, State::Forking, State::Running] {
            sessions.set_state("s1", to, 200).unwrap();
        }
        sessions.update("s1", |s| s.cid = Some(501)).unwrap();
        sessions.add_usage(
            501,
            &Usage {
                tokens_in: 7,
                tokens_out: 3,
                tokens_cached: 2,
                cost_micros: 40,
                model_calls: 1,
                tool_calls: 2,
            },
        );
        let (_dir, _store, evals) = fixture();
        evals.begin("s1", "change", 2, "build", 2);
        evals.hello(501, &["acp".to_string(), "harness=0.7.1".to_string()]);
        evals.tool(501, "read", 30, true);
        evals.tool(501, "run", 12, false);
        evals.tool(501, "memory_get", 1, true);
        let ended = sessions.finish("s1", "the step ended", false, 900).unwrap();

        let (begun, vm) = evals.take(&ended);
        let run = Run::of(&ended, begun, vm);
        assert_eq!((run.id, run.session.as_str()), (1, "s1"));
        assert_eq!(
            (run.provider.as_deref(), run.model.as_deref()),
            (Some("mock"), Some("mock/agent"))
        );
        assert_eq!(
            (run.recipe.as_str(), run.step.as_str(), run.seat.as_str()),
            ("change", "build", "builder")
        );
        assert_eq!((run.round, run.harness.as_str()), (2, "acp,harness=0.7.1"));
        assert_eq!(
            (run.recipe_version, run.harness_version.as_str()),
            (2, "0.7.1")
        );
        assert_eq!(
            (run.tokens_in, run.tokens_out, run.tokens_cached),
            (7, 3, 2)
        );
        assert_eq!(
            (run.cost_micros, run.model_calls, run.tool_calls),
            (40, 1, 2)
        );
        assert_eq!((run.tool_ms, run.wall_ms, run.memory_cites), (43, 700, 1));
        assert!(run.ok, "ended, and no reviewer rejected it");
        assert_eq!(run.gate, "none", "no reviewer gates the builder");
        // The VM's and the seat's entries are kept until the session ends, then dropped.
        assert_eq!(evals.take(&ended).1.tool_ms, 0);

        let rejected = Run::of(
            &Session {
                verdict: Some(false),
                ..ended
            },
            Begun::default(),
            Vm::default(),
        );
        assert!(!rejected.ok, "a reviewer's rejection is not a success");
        assert_eq!(rejected.gate, "failed");
    }

    #[test]
    fn a_session_nobody_began_is_recorded_as_a_change_step_of_its_role() {
        let sessions = Sessions::default();
        sessions.create(
            NewSession {
                project: "pr1".into(),
                task: Some("t1".into()),
                seat: Some("reviewer".into()),
                kind: crate::sessions::Kind::Step,
                parent: None,
                role: "reviewer".into(),
                engine: "harness".into(),
                model: None,
                attempt: 1,
            },
            5,
        );
        let failed = sessions
            .finish("s1", "its VM did not start in time", true, 9)
            .unwrap();
        let (_dir, _store, evals) = fixture();
        let (begun, _) = evals.take(&failed);
        let run = Run::of(&failed, begun, Vm::default());
        assert_eq!(
            (
                run.recipe.as_str(),
                run.step.as_str(),
                run.round,
                run.recipe_version
            ),
            ("change", "reviewer", 1, 2)
        );
        assert_eq!((run.provider, run.model), (None, None));
        assert_eq!(
            (run.outcome.as_str(), run.ok, run.gate.as_str()),
            ("its VM did not start in time", false, "failed")
        );
    }
}
