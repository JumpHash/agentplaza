//! Eval records (PLAN 25.1): every step session that ends leaves one `eval_run` row, what the eval suite and the owner's
//! scores read. The row is written through to `plaza.db` as the session ends ([`ended`]). What the session cost comes
//! from its record (usage, model, node, verdict); its tool time comes from the tool reports of its VM, which the hub
//! adds up as they arrive ([`Evals::tool`]); the later outcomes (landed) are set when the task's change lands
//! ([`Evals::landed`]).
//!
//! GET /v1/evals/runs?project=&model=&seat=&limit=  → [`Run`]s, newest first (limit 1..=200, default 50)
//! GET /v1/evals/scores                             → [`Score`]s per seat, model and engine
//! GET /v1/evals/tools                              → [`ToolLatency`] per tool, over the newest tool calls kept

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use apz_http::{Request, Response, RouteDoc, Router};
use apz_store::rusqlite::{Row, params};
use apz_store::table::Value;
use apz_store::{RowSet, StoreError, Table, TableDef};
use serde::{Deserialize, Serialize};

use crate::sessions::{Kind, Session};
use crate::state::App;
use crate::store::sql;

/// The newest runs a page returns by default, and the most one request may ask for.
pub const RUNS_PAGE: usize = 50;
pub const RUNS_LIMIT: usize = 200;
/// The newest tool calls whose durations the hub keeps (in memory: the latencies are for the hub's current life).
pub const TOOL_CALLS_KEPT: usize = 50_000;

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
}

/// Which runs to list. Each field is a match, and None matches any.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunFilter {
    pub project: Option<String>,
    pub model: Option<String>,
    pub seat: Option<String>,
}

/// How the runs of one seat, model and engine did.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Score {
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
    step: String,
    round: u32,
}

/// What the hub knows of a VM while its session runs: the capabilities it said hello with, and its tools' time.
#[derive(Debug, Clone, Default)]
struct Vm {
    harness: String,
    tool_ms: u64,
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

    /// A session started for a seat of `recipe` running `step` in its loop `round`: its record keeps them.
    pub fn begin(&self, session: &str, recipe: &str, step: &str, round: u32) {
        self.begun().insert(
            session.to_string(),
            Begun {
                recipe: recipe.to_string(),
                step: step.to_string(),
                round,
            },
        );
    }

    /// VM `cid` said hello with `caps`.
    pub fn hello(&self, cid: u64, caps: &[String]) {
        self.vms().entry(cid).or_default().harness = caps.join(",");
    }

    /// A tool of VM `cid` ran for `ms` milliseconds, and did (`ok`) or failed.
    pub fn tool(&self, cid: u64, tool: &str, ms: u64, ok: bool) {
        self.vms().entry(cid).or_default().tool_ms += ms;
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
        self.db.write(move |conn| insert_run(conn, &run))?;
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

    /// How the runs of each seat, model and engine did.
    pub fn scores(&self) -> Vec<Score> {
        let read = self.db.read(|conn| {
            let mut stmt = conn.prepare_cached(sql::EVAL_SCORES)?;
            let scores = stmt
                .query_map([], |r| {
                    let runs: u64 = r.get(3)?;
                    let successes: u64 = r.get(4)?;
                    let cost: u64 = r.get(5)?;
                    let wall: u64 = r.get(6)?;
                    Ok(Score {
                        seat: r.get(0)?,
                        model: r.get(1)?,
                        engine: r.get(2)?,
                        runs,
                        successes,
                        success_rate: successes as f64 / runs.max(1) as f64,
                        cost_per_success_micros: per_success(cost, successes),
                        time_per_success_ms: per_success(wall, successes),
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

    /// The record of `session` ends with what its VM did: the in-flight entries are dropped.
    fn take(&self, session: &Session) -> (Begun, Vm) {
        let begun = self.begun().remove(&session.id).unwrap_or_else(|| Begun {
            recipe: "change".to_string(),
            step: session.role.clone(),
            round: session.attempt,
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
            ok: session.state == crate::sessions::State::Ended && session.verdict != Some(false),
            landed: None,
            reverted: None,
            ended_ms,
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

/// The runs' table (`eval_run`), written through.
pub fn def() -> &'static TableDef {
    apz_store::root_def!(Run)
}

/// A count or an id as a SQL parameter (values past i64 are clamped; no row holds them).
fn bound(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Writes one run as a row of `eval_run`, keyed by its id (the table's `pos` takes the id too: a stream has no list
/// position of its own).
fn insert_run(conn: &apz_store::rusqlite::Connection, run: &Run) -> Result<(), StoreError> {
    let def = def();
    let mut rows = RowSet::new();
    run.put(def, vec![Value::Integer(bound(run.id))], &mut rows)?;
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
    })
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .get("/v1/evals/runs", |app, req, _| runs_route(app, req))
        .doc(RouteDoc::new("List the eval records of step sessions, newest first").errors(&[400]))
        .get("/v1/evals/scores", |app, _, _| {
            Response::json(200, &app.evals.scores())
        })
        .doc(RouteDoc::new(
            "Score the step sessions by seat, model and engine",
        ))
        .get("/v1/evals/tools", |app, _, _| {
            Response::json(200, &app.evals.tool_latency())
        })
        .doc(RouteDoc::new(
            "Show each tool's calls, errors and latency (p50, p95)",
        ))
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
    let text = |name: &str| {
        req.query_param(name)
            .map(crate::plaza::percent_decode)
            .filter(|v| !v.is_empty())
    };
    let filter = RunFilter {
        project: text("project"),
        model: text("model"),
        seat: text("seat"),
    };
    Response::json(200, &app.evals.runs(&filter, limit))
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

    /// The record of session `id` of `task`'s `seat`, run on `model`.
    fn run(id: u64, task: &str, seat: &str, model: &str, ok: bool, cost: u64, wall: u64) -> Run {
        Run {
            id,
            session: format!("s{id}"),
            project: "pr1".into(),
            task: Some(task.into()),
            recipe: "change".into(),
            step: "build".into(),
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
        }
    }

    fn ids(runs: &[Run]) -> Vec<u64> {
        runs.iter().map(|r| r.id).collect()
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
    fn scores_count_the_successes_and_their_cost_and_time_per_seat_model_and_engine() {
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
        assert_eq!((builder.runs, builder.successes), (3, 2));
        assert!((builder.success_rate - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(builder.cost_per_success_micros, Some(125.0));
        assert_eq!(builder.time_per_success_ms, Some(1250.0));
        let reviewer = &scores[1];
        assert_eq!((reviewer.runs, reviewer.successes), (1, 0));
        assert_eq!(
            (reviewer.success_rate, reviewer.cost_per_success_micros),
            (0.0, None),
            "no success, no cost per success"
        );
    }

    #[test]
    fn a_session_record_takes_its_usage_model_and_tool_time_from_the_session_and_its_vm() {
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
        evals.begin("s1", "change", "build", 2);
        evals.hello(501, &["acp".to_string()]);
        evals.tool(501, "read", 30, true);
        evals.tool(501, "run", 12, false);
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
        assert_eq!((run.round, run.harness.as_str()), (2, "acp"));
        assert_eq!(
            (run.tokens_in, run.tokens_out, run.tokens_cached),
            (7, 3, 2)
        );
        assert_eq!(
            (run.cost_micros, run.model_calls, run.tool_calls),
            (40, 1, 2)
        );
        assert_eq!((run.tool_ms, run.wall_ms), (42, 700));
        assert!(run.ok, "ended, and no reviewer rejected it");
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
            (run.recipe.as_str(), run.step.as_str(), run.round),
            ("change", "reviewer", 1)
        );
        assert_eq!((run.provider, run.model), (None, None));
        assert_eq!(
            (run.outcome.as_str(), run.ok),
            ("its VM did not start in time", false)
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
}
