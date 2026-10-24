//! What a lab run observes (PLAN 22.7): the OTLP receiver its collector posts to, the logs and spans it keeps with the
//! run, its metric series in the metrics store, the topology's assertions checked over them (a failing one is a finding
//! on the change's review tree, citing the evidence), and the tools sessions query the observations with.
//!
//! The collector (apz-topology's `apz-topo-collect`, in the lab VM) posts with the run's ingest token in
//! `X-Apz-Lab-Token`; the run keeps only the token's hash. A run takes observations until it is done.
//!
//! Routes:
//!   POST /v1/test-lab/runs/{id}/otlp/{logs|traces|metrics}  the collector's OTLP (JSON), or its log lines as OTLP logs
//!   GET  /v1/test-lab/runs/{id}/observations                the run's counts and the verdicts its assessment gave
//!   POST /v1/test-lab/runs/{id}/assess                      {topology?} checks the topology's assertions on a done run
//!
//! Tools (sessions, of their project's runs): `logs_query`, `metrics_query`, `traces_query` (see [`TOOLS`]).

use std::collections::BTreeMap;

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_metrics::agg::{Agg, Tier};
use apz_metrics::{EntityKind, Sample, SeriesKey};
use apz_topology::assert::{self, Evidence, REQUESTS, Verdict as Check};
use apz_topology::collector::TOKEN_HEADER;
use apz_topology::compare::Limit;
use apz_topology::observe::{self, LogLine, PointKind, SpanRec};
use apz_topology::resilience;
use apz_topology::spec::{self, Assertion, Scenario, Topology};
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sha2::{Digest, Sha256};

use crate::agents::Agent;
use crate::lab_performance;
use crate::lab_security;
use crate::metrics::Metrics;
use crate::releases::entity::Release;
use crate::review_graph::{By, Finding, Reviews, Verdict as FindingVerdict};
use crate::state::App;
use crate::test_lab::{LabRun, Store};

/// The tools this module serves to sessions.
pub const TOOLS: &[&str] = &["logs_query", "metrics_query", "traces_query"];
/// The most log lines and spans one run keeps; the rest are counted as dropped.
pub const MAX_OBSERVED: u64 = 100_000;
/// The largest body a run's collector may post (its batches of log lines and traces are bigger than a public call's).
pub const OTLP_BODY: usize = 8 << 20;

/// A log line a run kept, with its place in the run (`seq`), its severity and its attributes as `key=value`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct LabLog {
    pub run: String,
    pub seq: u64,
    pub t_ms: u64,
    pub service: String,
    pub severity: String,
    pub body: String,
    #[serde(default)]
    pub trace_id: String,
    #[serde(default)]
    pub span_id: String,
    #[serde(default)]
    #[table(list = "test_lab_log_attr")]
    pub attrs: Vec<String>,
}

/// A span a run kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct LabSpan {
    pub run: String,
    pub seq: u64,
    pub trace_id: String,
    pub span_id: String,
    #[serde(default)]
    pub parent_id: String,
    pub name: String,
    pub service: String,
    pub kind: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub status: String,
    #[serde(default)]
    #[table(list = "test_lab_span_attr")]
    pub attrs: Vec<String>,
}

/// One assertion's verdict on a run: whether it passed, what it found, its evidence, and the finding it became (the
/// review run's id and the finding's id, `<review>/<f<n>>`; empty when it passed or no review run exists yet).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, apz_store::Table)]
pub struct LabVerdict {
    pub run: String,
    pub id: String,
    pub kind: String,
    pub passed: bool,
    pub detail: String,
    #[serde(default)]
    pub finding: String,
    #[serde(default)]
    #[table(list = "test_lab_verdict_cite")]
    pub cites: Vec<String>,
}

/// What one ingest kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Accepted {
    pub records: usize,
    pub dropped: usize,
}

/// A fresh ingest token: 32 random bytes, in hex. Its collector gets it once.
pub fn new_token() -> String {
    hex(&apz_trust::random::<32>())
}

/// The hash a run keeps of its token, in hex.
pub fn token_hash(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Whether a request is a lab run's collector posting its observations: a public route, checked by its token.
pub fn is_otlp(method: &str, path: &str) -> bool {
    method == "POST" && otlp_parts(path).is_some()
}

/// The run id and the signal of `/v1/test-lab/runs/{id}/otlp/{signal}`.
fn otlp_parts(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("/v1/test-lab/runs/")?;
    let (id, tail) = rest.split_once('/')?;
    let signal = tail.strip_prefix("otlp/")?;
    (!id.is_empty() && matches!(signal, "logs" | "traces" | "metrics")).then_some((id, signal))
}

/// The room a run has for `n` more records, given what it kept (pure).
fn room(observed: u64, n: usize) -> usize {
    usize::try_from(MAX_OBSERVED.saturating_sub(observed))
        .unwrap_or(usize::MAX)
        .min(n)
}

/// Takes what a run's collector posts to `signal` (PLAN 22.7). The token is checked first, then the run's state; the
/// logs and spans are kept with the run (past [`MAX_OBSERVED`] they are counted as dropped), and the metric points go
/// into the metrics store as the run's series: a gauge as read, a counter as its increase (a cumulative total as the
/// step from its last total).
pub fn ingest(
    lab: &Store,
    metrics: &Metrics,
    run: &str,
    token: &str,
    signal: &str,
    raw: &[u8],
    now_ms: u64,
) -> Result<Accepted, (u16, String)> {
    let mut p = lock(&lab.inner);
    let at = p
        .runs
        .iter()
        .position(|r| r.id == run)
        .ok_or((404, "no such run".to_string()))?;
    if !apz_trust::ct_eq(
        token_hash(token).as_bytes(),
        p.runs[at].token_sha.as_bytes(),
    ) {
        return Err((401, "the run's ingest token is not this one".into()));
    }
    if p.runs[at].state == "done" {
        return Err((
            409,
            "that run is done: it takes no more observations".into(),
        ));
    }
    let body: Json =
        serde_json::from_slice(raw).map_err(|e| (400, format!("the body is not JSON: {e}")))?;
    match signal {
        "logs" => {
            let lines = observe::parse_logs(&body).map_err(|e| (400, e))?;
            let taken = room(p.runs[at].observed, lines.len());
            let base = p.runs[at].observed;
            for (i, line) in lines.iter().take(taken).enumerate() {
                p.logs.push(LabLog {
                    run: run.into(),
                    seq: base + i as u64,
                    t_ms: line.t_ms,
                    service: line.service.clone(),
                    severity: line.severity.clone(),
                    body: line.body.clone(),
                    trace_id: line.trace_id.clone(),
                    span_id: line.span_id.clone(),
                    attrs: attrs_rows(&line.attrs),
                });
            }
            let dropped = lines.len() - taken;
            p.runs[at].observed += taken as u64;
            p.runs[at].dropped += dropped as u64;
            Ok(Accepted {
                records: taken,
                dropped,
            })
        }
        "traces" => {
            let spans = observe::parse_traces(&body).map_err(|e| (400, e))?;
            let taken = room(p.runs[at].observed, spans.len());
            let base = p.runs[at].observed;
            for (i, span) in spans.iter().take(taken).enumerate() {
                p.spans.push(LabSpan {
                    run: run.into(),
                    seq: base + i as u64,
                    trace_id: span.trace_id.clone(),
                    span_id: span.span_id.clone(),
                    parent_id: span.parent_id.clone(),
                    name: span.name.clone(),
                    service: span.service.clone(),
                    kind: span.kind.clone(),
                    start_ms: span.start_ms,
                    end_ms: span.end_ms,
                    status: span.status.clone(),
                    attrs: attrs_rows(&span.attrs),
                });
            }
            let dropped = spans.len() - taken;
            p.runs[at].observed += taken as u64;
            p.runs[at].dropped += dropped as u64;
            Ok(Accepted {
                records: taken,
                dropped,
            })
        }
        "metrics" => {
            let points = observe::parse_metrics(&body).map_err(|e| (400, e))?;
            let mut totals = lock(&lab.totals);
            let mut samples = Vec::with_capacity(points.len());
            for point in &points {
                let key = SeriesKey::new(&point.name, EntityKind::Run, run)
                    .label("service", &point.service);
                let value = match point.kind {
                    PointKind::Gauge => apz_metrics::Value::Gauge(point.value),
                    PointKind::Delta => apz_metrics::Value::Counter(point.value),
                    PointKind::Cumulative => {
                        let last = totals.insert(key.id(), point.value);
                        let increase = match last {
                            Some(prev) if point.value >= prev => point.value - prev,
                            _ => point.value,
                        };
                        apz_metrics::Value::Counter(increase)
                    }
                };
                let t_ms = if point.t_ms == 0 { now_ms } else { point.t_ms };
                samples.push(Sample { key, t_ms, value });
            }
            drop(totals);
            drop(p);
            crate::metrics::ingest::samples(metrics, &samples, now_ms);
            Ok(Accepted {
                records: points.len(),
                dropped: 0,
            })
        }
        _ => Err((404, "no such signal".into())),
    }
}

/// Attributes as the rows keep them: `key=value`.
fn attrs_rows(attrs: &[(String, String)]) -> Vec<String> {
    attrs.iter().map(|(k, v)| format!("{k}={v}")).collect()
}

/// The attributes of a row (`key=value`, split at the first `=`).
fn attrs_of(rows: &[String]) -> Vec<(String, String)> {
    rows.iter()
        .map(|row| {
            row.split_once('=').map_or_else(
                || (row.clone(), String::new()),
                |(k, v)| (k.to_string(), v.to_string()),
            )
        })
        .collect()
}

fn log_of(line: &LabLog) -> LogLine {
    LogLine {
        t_ms: line.t_ms,
        service: line.service.clone(),
        severity: line.severity.clone(),
        body: line.body.clone(),
        trace_id: line.trace_id.clone(),
        span_id: line.span_id.clone(),
        attrs: attrs_of(&line.attrs),
    }
}

fn span_of(span: &LabSpan) -> SpanRec {
    SpanRec {
        trace_id: span.trace_id.clone(),
        span_id: span.span_id.clone(),
        parent_id: span.parent_id.clone(),
        name: span.name.clone(),
        service: span.service.clone(),
        kind: span.kind.clone(),
        start_ms: span.start_ms,
        end_ms: span.end_ms,
        status: span.status.clone(),
        attrs: attrs_of(&span.attrs),
    }
}

/// The windows of a metric's series of a run, from the store (the closed ones) and the rollup (the open ones), each
/// series with its windows in time order.
pub(crate) fn windows(
    metrics: &Metrics,
    run: &str,
    metric: &str,
    from_ms: u64,
    to_ms: u64,
) -> Vec<(SeriesKey, Vec<(u64, Agg)>)> {
    let keep = |key: &SeriesKey| key.kind == EntityKind::Run && key.entity == run;
    let mut out = metrics
        .store
        .rows(Tier::Raw10s, metric, from_ms, to_ms, &keep);
    {
        let rollup = crate::metrics::lock(&metrics.rollup);
        for (key, start, agg) in rollup.open_at(Tier::Raw10s) {
            if key.metric != metric || !keep(key) || !(from_ms..to_ms).contains(&start) {
                continue;
            }
            match out.iter_mut().find(|(k, _)| k == key) {
                Some((_, rows)) => rows.push((start, agg.clone())),
                None => out.push((key.clone(), vec![(start, agg.clone())])),
            }
        }
    }
    for (_, rows) in &mut out {
        rows.sort_by_key(|(start, _)| *start);
    }
    out
}

/// What a run observed, as the assertions read it: its logs and spans, and each named series' windows (the sums of
/// each window, every service's together).
pub fn evidence(lab: &Store, metrics: &Metrics, run: &LabRun, names: &[String]) -> Evidence {
    let (logs, spans) = {
        let p = lock(&lab.inner);
        let logs = p
            .logs
            .iter()
            .filter(|l| l.run == run.id)
            .map(log_of)
            .collect();
        let spans = p
            .spans
            .iter()
            .filter(|s| s.run == run.id)
            .map(span_of)
            .collect();
        (logs, spans)
    };
    let mut series = BTreeMap::new();
    for name in names {
        let mut points: Vec<(u64, f64)> = windows(metrics, &run.id, name, 0, i64::MAX as u64)
            .into_iter()
            .flat_map(|(_, rows)| rows.into_iter().map(|(t, agg)| (t, agg.sum)))
            .collect();
        points.sort_by_key(|(t, _)| *t);
        series.insert(name.clone(), points);
    }
    Evidence {
        started_ms: run.started_ms.unwrap_or(run.enqueued_ms),
        logs,
        spans,
        series,
    }
}

/// The series an assessment reads: the requests and each hit rate's two series.
fn metric_names(assertions: &[Assertion]) -> Vec<String> {
    let mut names = vec![REQUESTS.to_string()];
    for assertion in assertions {
        if let Assertion::HitRateAbove { hits, misses, .. } = assertion {
            names.push(hits.clone());
            names.push(misses.clone());
        }
    }
    names.sort();
    names.dedup();
    names
}

/// The finding kind a failing resilience scenario is filed as (PLAN 22.8): the change tree reads it like `performance`.
pub const RESILIENCE_KIND: &str = "resilience";

/// The finding a failing step becomes on its change's review tree (PLAN 22.7): a confirmed finding of kind
/// `observability` whose proof is the evidence it cites. Empty when the run names no change or the change has no review
/// run yet.
fn file_finding(reviews: &Reviews, run: &LabRun, topology: &Topology, check: &Check) -> String {
    if run.change.is_empty() {
        return String::new();
    }
    let said = topology
        .assertions
        .iter()
        .find(|a| a.id() == check.id)
        .map_or_else(String::new, Assertion::describe);
    let by = By {
        seat: "test-lab".into(),
        model: "assertion".into(),
        session: run.id.clone(),
    };
    let proof = if check.cites.is_empty() {
        format!("lab run {}: {}", run.id, check.detail)
    } else {
        format!("lab run {}\n{}", run.id, check.cites.join("\n"))
    };
    let finding = Finding {
        symbol: format!("{}/{}.toml", spec::DIR, topology.name),
        lineage: None,
        text: format!("{} failed: {}", check.id, check.detail),
        severity: "major".into(),
        node: String::new(),
        round: 0,
        id: String::new(),
        scenario: format!(
            "lab run {} of topology {} observed: {said}",
            run.id, topology.name
        ),
        kind: "observability".into(),
        verdict: FindingVerdict::Confirmed,
        proof,
        reason: String::new(),
        by: Some(by.clone()),
        verified_by: Some(by),
        contest: String::new(),
        ruling: String::new(),
        ruling_reason: String::new(),
    };
    reviews
        .attach_finding(&run.change, finding)
        .map_or_else(String::new, |(review, id)| format!("{review}/{id}"))
}

/// The finding a failing resilience scenario becomes on its change's review tree (PLAN 22.8): a confirmed finding of kind
/// `resilience` whose proof is the numbers the scenario was judged by. Empty when the run names no change.
fn file_resilience(
    reviews: &Reviews,
    run: &LabRun,
    topology: &Topology,
    scenario: &Scenario,
    check: &Check,
) -> String {
    if run.change.is_empty() {
        return String::new();
    }
    let by = By {
        seat: "test-lab".into(),
        model: "resilience".into(),
        session: run.id.clone(),
    };
    let finding = Finding {
        symbol: format!("{}/{}.toml", spec::DIR, topology.name),
        lineage: None,
        text: format!("{} failed: {}", check.id, check.detail),
        severity: "major".into(),
        node: String::new(),
        round: 0,
        id: String::new(),
        scenario: format!(
            "lab run {} of topology {}: {:?} on {} for {} s, probed at {}{}, must recover within {} s and fail at most {} % of its probes",
            run.id,
            topology.name,
            scenario.action,
            scenario.target,
            scenario.for_secs,
            scenario.probe.service,
            scenario.probe.path,
            scenario.recover_within_secs,
            scenario.expect.max_error_percent
        ),
        kind: RESILIENCE_KIND.into(),
        verdict: FindingVerdict::Confirmed,
        proof: format!("lab run {}\n{}", run.id, check.detail),
        reason: String::new(),
        by: Some(by.clone()),
        verified_by: Some(by),
        contest: String::new(),
        ruling: String::new(),
        ruling_reason: String::new(),
    };
    reviews
        .attach_finding(&run.change, finding)
        .map_or_else(String::new, |(review, id)| format!("{review}/{id}"))
}

/// Stores one verdict with its run (a re-assessment replaces it). A failing one is filed once as a finding: `file`
/// files it, and a finding that an earlier assessment filed is kept.
pub(crate) fn record(
    lab: &Store,
    run: &LabRun,
    check: &Check,
    file: impl FnOnce() -> String,
) -> LabVerdict {
    let prior = lock(&lab.inner)
        .verdicts
        .iter()
        .find(|v| v.run == run.id && v.id == check.id)
        .cloned();
    let finding = match &prior {
        Some(prior) if !prior.finding.is_empty() => prior.finding.clone(),
        _ if check.passed => String::new(),
        _ => file(),
    };
    let row = LabVerdict {
        run: run.id.clone(),
        id: check.id.clone(),
        kind: check.kind.clone(),
        passed: check.passed,
        detail: check.detail.clone(),
        finding,
        cites: check.cites.clone(),
    };
    {
        let mut p = lock(&lab.inner);
        p.verdicts.retain(|v| !(v.run == run.id && v.id == row.id));
        p.verdicts.push(row.clone());
    }
    row
}

/// Checks the topology's assertions over a done run (PLAN 22.7), and its performance against its release's baseline
/// under the project's `limits` (PLAN 22.8; `releases` are the project's, newest first). Each verdict is stored with the
/// run, and a failing one is filed once as a finding on the run's change.
pub fn assess(
    lab: &Store,
    metrics: &Metrics,
    reviews: &Reviews,
    run_id: &str,
    topology: &Topology,
    releases: &[Release],
    limits: &[Limit],
) -> Result<Vec<LabVerdict>, (u16, String)> {
    let run = lab
        .runs()
        .into_iter()
        .find(|r| r.id == run_id)
        .ok_or((404, "no such run".to_string()))?;
    if run.state != "done" {
        return Err((
            409,
            "that run is not done: it is assessed when it ends".into(),
        ));
    }
    let evidence = evidence(lab, metrics, &run, &metric_names(&topology.assertions));
    let mut out = Vec::new();
    for check in assert::check(&topology.assertions, &evidence) {
        out.push(record(lab, &run, &check, || {
            file_finding(reviews, &run, topology, &check)
        }));
    }
    if let Some(outcome) = lab_performance::compare_run(lab, metrics, releases, &run, limits) {
        for comparison in &outcome.comparisons {
            let check = Check {
                id: format!("perf:{}", comparison.metric),
                kind: lab_performance::KIND.into(),
                passed: comparison.passed,
                detail: lab_performance::describe(comparison),
                cites: vec![format!(
                    "baseline {} run {}",
                    outcome.baseline.release, outcome.baseline.run
                )],
            };
            out.push(record(lab, &run, &check, || {
                lab_performance::file_finding(reviews, &run, &outcome, comparison)
            }));
        }
    }
    // Each resilience scenario is judged by the numbers its run recorded (PLAN 22.8). A scenario with no numbers did not
    // run here: its verdict fails it with the reason, and files nothing, since nothing was observed to be wrong.
    for scenario in &topology.resilience {
        let names = resilience::names(&scenario.id);
        let values = lab_performance::values(metrics, &run.id, &names);
        let (check, measured) = match resilience::from_metrics(&scenario.id, &values) {
            Some(measured) => {
                let judgment = resilience::judge(scenario, &measured);
                let check = Check {
                    id: format!("resilience:{}", scenario.id),
                    kind: RESILIENCE_KIND.into(),
                    passed: judgment.passed,
                    detail: judgment.detail,
                    cites: vec![format!("lab run {} recorded {}", run.id, names.join(", "))],
                };
                (check, true)
            }
            None => (
                Check {
                    id: format!("resilience:{}", scenario.id),
                    kind: RESILIENCE_KIND.into(),
                    passed: false,
                    detail: format!(
                        "{}: not run, no measurement was recorded at this run",
                        scenario.id
                    ),
                    cites: Vec::new(),
                },
                false,
            ),
        };
        out.push(record(lab, &run, &check, || {
            if measured {
                file_resilience(reviews, &run, topology, scenario, &check)
            } else {
                String::new()
            }
        }));
    }
    Ok(out)
}

/// What a run observed, for `GET .../observations`: its state, the counts of its logs, errors and spans, and its
/// verdicts. None for an unknown run.
pub fn observations(lab: &Store, run_id: &str) -> Option<Json> {
    let run = lab.runs().into_iter().find(|r| r.id == run_id)?;
    let p = lock(&lab.inner);
    let logs: Vec<&LabLog> = p.logs.iter().filter(|l| l.run == run_id).collect();
    let errors = logs
        .iter()
        .filter(|l| observe::is_error(&l.severity))
        .count();
    let spans = p.spans.iter().filter(|s| s.run == run_id).count();
    let verdicts: Vec<&LabVerdict> = p.verdicts.iter().filter(|v| v.run == run_id).collect();
    Some(json!({
        "run": run_id,
        "state": run.state,
        "observed": run.observed,
        "dropped": run.dropped,
        "logs": logs.len(),
        "errors": errors,
        "spans": spans,
        "verdicts": verdicts,
    }))
}

/// Runs a session's call of one of [`TOOLS`] over the runs of `project`. A run of another project is refused as unknown
/// to the project; the answer is JSON text.
pub fn answer(
    lab: &Store,
    metrics: &Metrics,
    project: &str,
    tool: &str,
    input: &Json,
) -> Result<String, String> {
    let run_id = opt_str(input, "run")?
        .filter(|id| !id.is_empty())
        .ok_or("the call names its run")?;
    let run = lab
        .runs()
        .into_iter()
        .find(|r| r.id == run_id)
        .ok_or("no such run")?;
    if run.project != project {
        return Err(format!("run {run_id} is not a run of this project"));
    }
    let answer = match tool {
        "logs_query" => logs_query(lab, &run, input)?,
        "metrics_query" => metrics_query(metrics, &run, input)?,
        "traces_query" => traces_query(lab, &run, input)?,
        other => return Err(format!("{other} is not a lab observation tool")),
    };
    Ok(answer.to_string())
}

/// Runs `name` (one of [`TOOLS`]) for the agent's project.
pub fn run(app: &App, agent: &Agent, name: &str, input: &Json) -> Result<String, String> {
    let project = crate::projects::project_of(app, agent.cid).ok_or("you are not on a project")?;
    answer(&app.test_lab, &app.metrics, &project, name, input)
}

fn opt_str<'a>(input: &'a Json, key: &str) -> Result<Option<&'a str>, String> {
    match input.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::String(text)) => Ok(Some(text.as_str())),
        Some(_) => Err(format!("{key} is a string")),
    }
}

fn u64_arg(input: &Json, key: &str, default: u64) -> Result<u64, String> {
    match input.get(key) {
        None | Some(Json::Null) => Ok(default),
        Some(value) => value
            .as_u64()
            .ok_or_else(|| format!("{key} is a whole number")),
    }
}

fn limit_arg(input: &Json, default: usize, max: usize) -> Result<usize, String> {
    let limit = u64_arg(input, "limit", default as u64)?;
    if limit == 0 || limit > max as u64 {
        return Err(format!("limit is 1 to {max}"));
    }
    Ok(limit as usize)
}

fn bool_arg(input: &Json, key: &str) -> Result<bool, String> {
    match input.get(key) {
        None | Some(Json::Null) => Ok(false),
        Some(Json::Bool(flag)) => Ok(*flag),
        Some(_) => Err(format!("{key} is true or false")),
    }
}

/// `logs_query`: the run's log lines matching the filters, the most recent `limit` of them, in time order.
fn logs_query(lab: &Store, run: &LabRun, input: &Json) -> Result<Json, String> {
    let service = opt_str(input, "service")?;
    let severity = opt_str(input, "severity")?.map(str::to_ascii_uppercase);
    let contains = opt_str(input, "contains")?;
    let limit = limit_arg(input, 100, 500)?;
    let p = lock(&lab.inner);
    let matching: Vec<&LabLog> = p
        .logs
        .iter()
        .filter(|l| l.run == run.id)
        .filter(|l| service.is_none_or(|s| l.service == s))
        .filter(|l| severity.as_deref().is_none_or(|s| l.severity == s))
        .filter(|l| contains.is_none_or(|c| l.body.contains(c)))
        .collect();
    let total = matching.len();
    let lines: Vec<Json> = matching
        .iter()
        .rev()
        .take(limit)
        .rev()
        .map(|l| {
            json!({
                "seq": l.seq, "t_ms": l.t_ms, "service": l.service, "severity": l.severity,
                "body": l.body, "trace_id": l.trace_id, "attrs": attrs_of(&l.attrs),
            })
        })
        .collect();
    Ok(json!({"run": run.id, "total": total, "returned": lines.len(), "lines": lines}))
}

/// `metrics_query`: a metric's series of the run, each window as `[start_ms, sum, count, last, min, max]`.
fn metrics_query(metrics: &Metrics, run: &LabRun, input: &Json) -> Result<Json, String> {
    let metric = opt_str(input, "metric")?
        .filter(|m| !m.is_empty())
        .ok_or("metrics_query names its metric")?;
    let service = opt_str(input, "service")?;
    let from = u64_arg(input, "from_ms", 0)?;
    let to = u64_arg(input, "to_ms", i64::MAX as u64)?;
    let series: Vec<Json> = windows(metrics, &run.id, metric, from, to)
        .into_iter()
        .filter(|(key, _)| service.is_none_or(|s| key.get("service") == Some(s)))
        .map(|(key, rows)| {
            let points: Vec<Json> = rows
                .iter()
                .map(|(start, a)| json!([start, a.sum, a.count, a.last, a.min, a.max]))
                .collect();
            json!({"service": key.get("service").unwrap_or_default(), "points": points})
        })
        .collect();
    Ok(json!({"run": run.id, "metric": metric, "series": series}))
}

/// `traces_query`: the run's spans matching the filters, the most recent `limit` of them.
fn traces_query(lab: &Store, run: &LabRun, input: &Json) -> Result<Json, String> {
    let trace = opt_str(input, "trace_id")?;
    let name = opt_str(input, "name")?;
    let service = opt_str(input, "service")?;
    let min_ms = u64_arg(input, "min_ms", 0)?;
    let errors_only = bool_arg(input, "errors_only")?;
    let limit = limit_arg(input, 50, 500)?;
    let p = lock(&lab.inner);
    let matching: Vec<&LabSpan> = p
        .spans
        .iter()
        .filter(|s| s.run == run.id)
        .filter(|s| trace.is_none_or(|t| s.trace_id == t))
        .filter(|s| name.is_none_or(|n| s.name == n))
        .filter(|s| service.is_none_or(|v| s.service == v))
        .filter(|s| s.end_ms.saturating_sub(s.start_ms) >= min_ms)
        .filter(|s| !errors_only || s.status == "error")
        .collect();
    let total = matching.len();
    let spans: Vec<Json> = matching
        .iter()
        .rev()
        .take(limit)
        .rev()
        .map(|s| {
            json!({
                "trace_id": s.trace_id, "span_id": s.span_id, "parent_id": s.parent_id,
                "name": s.name, "service": s.service, "kind": s.kind, "start_ms": s.start_ms,
                "duration_ms": s.end_ms.saturating_sub(s.start_ms), "status": s.status,
                "attrs": attrs_of(&s.attrs),
            })
        })
        .collect();
    Ok(json!({"run": run.id, "total": total, "returned": spans.len(), "spans": spans}))
}

pub fn routes(router: Router<App>) -> Router<App> {
    let router = router
        .post("/v1/test-lab/runs/{id}/otlp/{signal}", otlp_route)
        .doc(RouteDoc::new("A lab run's collector posts its observations: OTLP as JSON (logs, traces or metrics), with the run's ingest token in X-Apz-Lab-Token").errors(&[400, 401, 404, 409]))
        .get("/v1/test-lab/runs/{id}/observations", observations_route)
        .doc(RouteDoc::new("What a lab run observed: its counts of logs, errors and spans, and the verdicts its assessment gave").errors(&[404]))
        .post("/v1/test-lab/runs/{id}/assess", assess_route)
        .doc(RouteDoc::new("Checks a done run's topology assertions over what it observed, its performance against its release's baseline, and the outcomes of its security stages posted in the body (`security`: what apz-security printed; a stage that did not run is a verdict that did not pass); each failing step is filed as a finding on the run's change").errors(&[400, 404, 409]));
    lab_performance::routes(router)
}

fn otlp_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let Some((_, signal)) = otlp_parts(&req.path) else {
        return Response::error(404, "no such signal");
    };
    let token = req.header(TOKEN_HEADER).unwrap_or_default();
    match ingest(
        &app.test_lab,
        &app.metrics,
        p.get("id"),
        token,
        signal,
        &req.body,
        apz_plaza::now_ms(),
    ) {
        Ok(accepted) => Response::json(200, &accepted),
        Err((status, message)) => Response::error(status, &message),
    }
}

fn observations_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    match observations(&app.test_lab, p.get("id")) {
        Some(view) => Response::json(200, &view),
        None => Response::error(404, "no such run"),
    }
}

/// The body of `POST .../assess`: the topology's TOML, or none to read the run's topology from its project's repository.
#[derive(Debug, Default, Deserialize)]
struct AssessBody {
    #[serde(default)]
    topology: Option<String>,
    /// The outcomes of the run's security stages, as apz-security prints them (PLAN 22.8): judged with its assessment.
    #[serde(default)]
    security: Vec<apz_topology::security::Outcome>,
}

fn assess_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let body: AssessBody = if req.body.is_empty() {
        AssessBody::default()
    } else {
        match serde_json::from_slice(&req.body) {
            Ok(body) => body,
            Err(e) => {
                return Response::error(400, &format!("the body is not a topology request: {e}"));
            }
        }
    };
    let id = p.get("id");
    let Some(run) = app.test_lab.runs().into_iter().find(|r| r.id == id) else {
        return Response::error(404, "no such run");
    };
    let security = body.security;
    let topology = match body.topology {
        Some(text) => match spec::parse(&text) {
            Ok(topology) => topology,
            Err(why) => return Response::error(400, &why),
        },
        None => {
            if run.topology.is_empty() {
                return Response::error(400, "the run names no topology to assess it by");
            }
            let found = match crate::topology::read(app, &run.project) {
                Ok(found) => found,
                Err(why) => return Response::error(400, &why),
            };
            match found
                .into_iter()
                .find_map(|f| f.topology.ok().filter(|t| t.name == run.topology))
            {
                Some(topology) => topology,
                None => {
                    return Response::error(
                        400,
                        &format!("topology {} is not in the project", run.topology),
                    );
                }
            }
        }
    };
    let (releases, limits) = lab_performance::context(app, &run.project);
    match assess(
        &app.test_lab,
        &app.metrics,
        &app.reviews,
        id,
        &topology,
        &releases,
        &limits,
    ) {
        Ok(mut verdicts) => {
            verdicts.extend(lab_security::assess(
                &app.test_lab,
                &app.reviews,
                &run,
                &topology,
                &security,
            ));
            Response::json(200, &json!({"run": id, "verdicts": verdicts}))
        }
        Err((status, message)) => Response::error(status, &message),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::review_graph::{NodeDef, NodeRun, NodeState, Run, RunState};
    use std::sync::Arc;

    /// Metrics over a fresh metrics file; the store is returned too, so it stays open while the test runs.
    pub(crate) fn metrics_in(name: &str) -> (apz_store::Store, Metrics) {
        let files = crate::metrics::sqlite::scratch_store(name);
        let m = Metrics::open(
            Arc::clone(files.db(crate::store::METRICS)),
            1_700_000_900_000,
        );
        (files, m)
    }

    /// A lab with one run of project `p`, observing topology `fake-run` for change `c1`, with its token.
    fn lab_with(id: &str, state: &str, token: &str) -> Store {
        let lab = Store::default();
        lock(&lab.inner).runs.push(LabRun {
            id: id.into(),
            project: "p".into(),
            channel: "stable".into(),
            state: state.into(),
            cpus: 1,
            memory_mib: 512,
            needs: Vec::new(),
            reservations: Vec::new(),
            enqueued_ms: 1_700_000_000_000,
            started_ms: Some(1_700_000_000_000),
            ended_ms: None,
            outcome: None,
            change: "c1".into(),
            topology: "fake-run".into(),
            release: String::new(),
            token_sha: token_hash(token),
            observed: 0,
            dropped: 0,
        });
        lab
    }

    fn set_state(lab: &Store, id: &str, state: &str) {
        for run in &mut lock(&lab.inner).runs {
            if run.id == id {
                run.state = state.into();
            }
        }
    }

    /// The change's review run: one sceptic node, so a finding has a place to go.
    pub(crate) fn review_of(change: &str) -> Run {
        let node = NodeRun {
            def: NodeDef {
                id: "sceptic".into(),
                kind: "agent".into(),
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
            id: format!("rv-{change}"),
            task: "t1".into(),
            project: "p".into(),
            worker: 1,
            change: change.into(),
            revision: "r1".into(),
            recipe: "review".into(),
            round: 1,
            rounds: 3,
            kind: "code".into(),
            state: RunState::Running,
            nodes: vec![node],
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

    pub(crate) fn reviews_in(dir: &tempfile::TempDir) -> Reviews {
        let blobs =
            apz_blobs::Blobs::open(&dir.path().join("blobs")).expect("the object store opens");
        Reviews::new(Arc::new(blobs))
    }

    /// The time the tests' runs observe at: long after their points, so their windows are closed.
    pub(crate) const NOW: u64 = 1_700_000_900_000;
    /// A topology with one service and the no-errors step.
    const NO_ERRORS: &str = "name = \"fake-run\"\n[[service]]\nname = \"api\"\nimage = \"api:1\"\n[[assert]]\nkind = \"no_errors\"\n";

    fn ingest_json(
        lab: &Store,
        metrics: &Metrics,
        token: &str,
        signal: &str,
        body: &Json,
    ) -> Result<Accepted, (u16, String)> {
        ingest(
            lab,
            metrics,
            "q1",
            token,
            signal,
            body.to_string().as_bytes(),
            NOW,
        )
    }

    #[test]
    fn a_seeded_error_fails_its_assertion_and_its_finding_cites_the_log() {
        let (_files, metrics) = metrics_in("lab-observe-error");
        let dir = tempfile::tempdir().unwrap();
        let reviews = reviews_in(&dir);
        reviews.put_run(review_of("c1"));
        let lab = lab_with("q1", "running", "run-token");

        let logs = observe::logs_otlp(
            "api-1",
            &[(
                1_700_000_000_000,
                "ERROR seeded failure in the cache".into(),
            )],
        );
        let accepted = ingest_json(&lab, &metrics, "run-token", "logs", &logs).unwrap();
        assert_eq!(
            accepted,
            Accepted {
                records: 1,
                dropped: 0
            }
        );
        set_state(&lab, "q1", "done");

        let topology = spec::parse(NO_ERRORS).unwrap();
        let verdicts = assess(&lab, &metrics, &reviews, "q1", &topology, &[], &[]).unwrap();
        assert_eq!(verdicts.len(), 1);
        assert!(!verdicts[0].passed);
        assert_eq!(
            verdicts[0].cites,
            ["log #0 api-1 ERROR: ERROR seeded failure in the cache"]
        );

        // The failure is a confirmed finding on the change's review tree, its proof the log line.
        let (review, id) = verdicts[0]
            .finding
            .split_once('/')
            .expect("the finding is filed");
        let run = reviews.run(review).unwrap();
        let finding = run
            .findings()
            .find(|f| f.id == id)
            .expect("the finding is on the run");
        assert_eq!(finding.verdict, FindingVerdict::Confirmed);
        assert_eq!(finding.kind, "observability");
        assert!(
            finding.proof.contains("seeded failure in the cache"),
            "{}",
            finding.proof
        );
        assert!(!finding.scenario.is_empty());

        // Assessing again replaces the verdict and does not file the finding twice.
        assess(&lab, &metrics, &reviews, "q1", &topology, &[], &[]).unwrap();
        assert_eq!(reviews.run(review).unwrap().findings().count(), 1);
        let shown = observations(&lab, "q1").unwrap();
        assert_eq!(shown["errors"], json!(1));
        assert_eq!(shown["verdicts"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_run_with_no_change_keeps_its_failing_verdict_without_a_finding() {
        let (_files, metrics) = metrics_in("lab-observe-nochange");
        let dir = tempfile::tempdir().unwrap();
        let reviews = reviews_in(&dir);
        let lab = lab_with("q1", "running", "run-token");
        lock(&lab.inner).runs[0].change.clear();
        let logs = observe::logs_otlp("api-1", &[(1_700_000_000_000, "ERROR boom".into())]);
        ingest_json(&lab, &metrics, "run-token", "logs", &logs).unwrap();
        set_state(&lab, "q1", "done");
        let topology = spec::parse(NO_ERRORS).unwrap();
        let verdicts = assess(&lab, &metrics, &reviews, "q1", &topology, &[], &[]).unwrap();
        assert!(!verdicts[0].passed);
        assert!(verdicts[0].finding.is_empty());
    }

    /// A topology with a cache-outage scenario: the cache is reset for 5 s, and the probe may fail 5 % at most.
    const OUTAGE: &str = "name = \"fake-run\"\n[[service]]\nname = \"api\"\nimage = \"api:1\"\n[[service]]\nname = \"cache\"\nimage = \"redis:7\"\n[[resilience]]\nid = \"cache-outage\"\naction = \"fault\"\ntarget = \"cache\"\nfault = { kind = \"reset\", percent = 30 }\nat_secs = 2\nfor_secs = 5\nrecover_within_secs = 10\nprobe = { service = \"api\", path = \"/items\" }\nexpect = { max_error_percent = 5.0 }\n";

    #[test]
    fn a_resilience_scenario_that_fails_files_a_finding_with_the_numbers_it_was_judged_by() {
        let (_files, metrics) = metrics_in("lab-observe-resilience");
        let dir = tempfile::tempdir().unwrap();
        let reviews = reviews_in(&dir);
        reviews.put_run(review_of("c1"));
        let lab = lab_with("q1", "running", "run-token");
        // The run's guest recorded what its probes saw: 10 of 30 failed while the cache was reset.
        let measured = resilience::Measured {
            requests: 30,
            failed: 10,
            error_percent: 33.3,
            p95_ms: 12.0,
            recovered_secs: Some(0.15),
        };
        let body = resilience::otlp("cache-outage", &measured, NOW);
        ingest_json(&lab, &metrics, "run-token", "metrics", &body).unwrap();
        set_state(&lab, "q1", "done");

        let topology = spec::parse(OUTAGE).unwrap();
        let verdicts = assess(&lab, &metrics, &reviews, "q1", &topology, &[], &[]).unwrap();
        let verdict = verdicts
            .iter()
            .find(|v| v.id == "resilience:cache-outage")
            .expect("the scenario is judged");
        assert!(!verdict.passed, "{verdict:?}");
        assert_eq!(verdict.kind, RESILIENCE_KIND);
        assert_eq!(
            verdict.detail,
            "cache-outage: 33.3 % of 30 probes failed while it held (the spec allows 5.0 %); recovered in 0.15 s (deadline 10 s). Not as the spec says: 33.3 % failed, over the 5.0 % the spec allows"
        );

        // Filed on the change's review tree, of kind resilience, its proof the numbers.
        let (review, id) = verdict
            .finding
            .split_once('/')
            .expect("the finding is filed");
        let run = reviews.run(review).unwrap();
        let finding = run.findings().find(|f| f.id == id).expect("on the run");
        assert_eq!(finding.kind, RESILIENCE_KIND);
        assert_eq!(finding.verdict, FindingVerdict::Confirmed);
        assert!(
            finding.proof.contains("33.3 % of 30 probes"),
            "{}",
            finding.proof
        );
        assert!(
            finding.scenario.contains("probed at api/items"),
            "{}",
            finding.scenario
        );

        // Assessing again keeps the one finding.
        assess(&lab, &metrics, &reviews, "q1", &topology, &[], &[]).unwrap();
        assert_eq!(reviews.run(review).unwrap().findings().count(), 1);
    }

    #[test]
    fn a_resilience_scenario_with_no_measurement_is_not_run_and_files_nothing() {
        let (_files, metrics) = metrics_in("lab-observe-not-run");
        let dir = tempfile::tempdir().unwrap();
        let reviews = reviews_in(&dir);
        reviews.put_run(review_of("c1"));
        let lab = lab_with("q1", "running", "run-token");
        set_state(&lab, "q1", "done");
        let topology = spec::parse(OUTAGE).unwrap();
        let verdicts = assess(&lab, &metrics, &reviews, "q1", &topology, &[], &[]).unwrap();
        let verdict = verdicts
            .iter()
            .find(|v| v.id == "resilience:cache-outage")
            .unwrap();
        assert!(!verdict.passed);
        assert!(verdict.detail.contains("not run"), "{}", verdict.detail);
        assert!(verdict.finding.is_empty());
        assert_eq!(reviews.run("rv-c1").unwrap().findings().count(), 0);
    }

    #[test]
    fn the_receiver_checks_the_token_and_refuses_a_done_run() {
        let (_files, metrics) = metrics_in("lab-observe-token");
        let lab = lab_with("q1", "running", "run-token");
        let logs = observe::logs_otlp("api-1", &[(1, "INFO fine".into())]);
        assert_eq!(
            ingest_json(&lab, &metrics, "not-the-token", "logs", &logs)
                .unwrap_err()
                .0,
            401
        );
        assert_eq!(
            ingest_json(&lab, &metrics, "run-token", "logs", &json!({}))
                .unwrap()
                .records,
            0
        );
        let refused = ingest(&lab, &metrics, "q9", "run-token", "logs", b"{}", NOW).unwrap_err();
        assert_eq!(refused.0, 404);
        assert_eq!(
            ingest_json(&lab, &metrics, "run-token", "sms", &json!({}))
                .unwrap_err()
                .0,
            404
        );
        assert_eq!(
            ingest(&lab, &metrics, "q1", "run-token", "logs", b"not json", NOW)
                .unwrap_err()
                .0,
            400
        );
        set_state(&lab, "q1", "done");
        assert_eq!(
            ingest_json(&lab, &metrics, "run-token", "logs", &logs)
                .unwrap_err()
                .0,
            409
        );
        assert!(is_otlp("POST", "/v1/test-lab/runs/q1/otlp/traces"));
        assert!(!is_otlp("GET", "/v1/test-lab/runs/q1/otlp/traces"));
        assert!(!is_otlp("POST", "/v1/test-lab/runs/q1/otlp/sms"));
    }

    #[test]
    fn cumulative_counters_land_in_the_metrics_store_as_their_increases() {
        let (_files, metrics) = metrics_in("lab-observe-counters");
        let lab = lab_with("q1", "running", "run-token");
        let total = |t: u64, value: &str| {
            json!({"resourceMetrics": [{"resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "cache-1"}}]},
                "scopeMetrics": [{"metrics": [{"name": "cache.hits", "sum": {"isMonotonic": true, "aggregationTemporality": 2,
                    "dataPoints": [{"timeUnixNano": (t * 1_000_000_000).to_string(), "asInt": value}]}}]}]}]})
        };
        // Both totals arrive while their window is still open (so the second adds to it, not over it).
        let near = 1_700_000_006_000;
        for (t, value) in [(1_700_000_000, "4"), (1_700_000_005, "10")] {
            let body = total(t, value).to_string();
            ingest(
                &lab,
                &metrics,
                "q1",
                "run-token",
                "metrics",
                body.as_bytes(),
                near,
            )
            .unwrap();
        }
        let names = vec!["cache.hits".to_string()];
        let evidence = evidence(&lab, &metrics, &lab_run(&lab), &names);
        let points = &evidence.series["cache.hits"];
        assert_eq!(
            points.iter().map(|(_, v)| v).sum::<f64>(),
            10.0,
            "4 then 6 more"
        );
    }

    fn lab_run(lab: &Store) -> LabRun {
        lab.runs().remove(0)
    }

    #[test]
    fn the_three_query_tools_answer_a_session_of_the_runs_project() {
        let (_files, metrics) = metrics_in("lab-observe-tools");
        let lab = lab_with("q1", "running", "run-token");
        let logs = observe::logs_otlp(
            "api-1",
            &[
                (1_700_000_000_000, "INFO served".into()),
                (
                    1_700_000_000_500,
                    "ERROR seeded failure in the cache".into(),
                ),
            ],
        );
        ingest_json(&lab, &metrics, "run-token", "logs", &logs).unwrap();
        let spans = json!({"resourceSpans": [{"resource": {"attributes": [{"key": "service.name", "value": {"stringValue": "api-1"}}]},
            "scopeSpans": [{"spans": [{"traceId": "0af7651916cd43dd8448eb211c80319c", "spanId": "b7ad6b7169203331",
                "name": "GET /items", "kind": 2, "startTimeUnixNano": "1700000000000000000",
                "endTimeUnixNano": "1700000000250000000", "status": {"code": 2}}]}]}]});
        ingest_json(&lab, &metrics, "run-token", "traces", &spans).unwrap();
        let hits = json!({"resourceMetrics": [{"resource": {"attributes": []}, "scopeMetrics": [{"metrics": [
            {"name": "http.server.requests", "sum": {"isMonotonic": true, "aggregationTemporality": 1,
                "dataPoints": [{"timeUnixNano": "1700000000000000000", "asInt": "3"}]}}]}]}]});
        ingest_json(&lab, &metrics, "run-token", "metrics", &hits).unwrap();

        let logs_out: Json = serde_json::from_str(
            &answer(
                &lab,
                &metrics,
                "p",
                "logs_query",
                &json!({"run": "q1", "severity": "error"}),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(logs_out["total"], json!(1));
        assert_eq!(
            logs_out["lines"][0]["body"],
            json!("ERROR seeded failure in the cache")
        );

        let traces_out: Json = serde_json::from_str(
            &answer(
                &lab,
                &metrics,
                "p",
                "traces_query",
                &json!({"run": "q1", "errors_only": true, "min_ms": 100}),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(traces_out["spans"][0]["duration_ms"], json!(250));

        let metrics_out: Json = serde_json::from_str(
            &answer(
                &lab,
                &metrics,
                "p",
                "metrics_query",
                &json!({"run": "q1", "metric": "http.server.requests"}),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(metrics_out["series"][0]["points"][0][1], json!(3.0));

        // Another project's run is not the project's to read, and an unknown tool is refused.
        assert!(answer(&lab, &metrics, "other", "logs_query", &json!({"run": "q1"})).is_err());
        assert!(answer(&lab, &metrics, "p", "logs_query", &json!({"run": "q9"})).is_err());
        assert!(
            answer(
                &lab,
                &metrics,
                "p",
                "logs_query",
                &json!({"run": "q1", "limit": 0})
            )
            .is_err()
        );
        assert!(answer(&lab, &metrics, "p", "sms_query", &json!({"run": "q1"})).is_err());
    }
}
