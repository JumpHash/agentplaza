//! The scrubbed production copy a lab run receives (PLAN 22.5, W59): the hub reads the project's scrubbing rules from
//! `.plaza/scrub.toml` on main, reads the source's connection from the vault (`connector:<source>`: the URL under
//! `SCRUB_SOURCE_URL`, and for a remote Postgres its CA under `SCRUB_SOURCE_CA`), and copies the source through the
//! scrubber into the run's directory. The copy stays only if the personal data scan passes; a copy that is kept belongs
//! to its run and is removed when the run ends ([`discard`], called by the lab's `finish`).
//!
//! Credentials never leave the hub: the lab gets the copy, not the URL. A `sqlite:<absolute path>` URL reads a SQLite
//! file (the owner's snapshot); any other URL is Postgres, reached as [`apz_topology::source::Postgres`] allows.
//!
//! Routes:
//!   POST /v1/test-lab/runs/{id}/scrub  {source} copies the source for the run. 201 with the scan's counts when the copy
//!                                      passed and is kept; 409 with the counts and no copy when the scan found data.
//!   GET  /v1/test-lab/runs/{id}/scrub  the copies the run asked for, with their outcomes and what their scans found.

use std::path::{Path, PathBuf};

use apz_http::{Params, Request, Response, RouteDoc, Router};
use apz_topology::scrub::{self, Refusal, Rules, Scrubber};
use apz_topology::source::{Postgres, Source, Sqlite};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::state::App;
use crate::test_lab::{LabScrub, LabScrubFinding};
use crate::vault::OnBehalf;

/// The vault name of a source's URL (bound to `connector:<source>`).
pub const URL_NAME: &str = "SCRUB_SOURCE_URL";
/// The vault name of a source's CA certificate, PEM (bound to `connector:<source>`), for a remote Postgres.
pub const CA_NAME: &str = "SCRUB_SOURCE_CA";
/// The prefix of a SQLite source's URL: the absolute path follows.
const SQLITE_PREFIX: &str = "sqlite:";

/// The directory a run's copies go in: beside the hub's state, and removed with the run.
pub fn run_dir(app: &App, run: &str) -> PathBuf {
    app.config.state_dir.join("lab-runs").join(run)
}

/// Removes everything a run kept: its copies. Called when the run ends (the lab's `finish`).
pub fn discard(app: &App, run: &str) {
    let _ = std::fs::remove_dir_all(run_dir(app, run));
}

pub fn routes(router: Router<App>) -> Router<App> {
    router
        .post("/v1/test-lab/runs/{id}/scrub", scrub_route)
        .doc(RouteDoc::new("Copies a source for a run through the project's scrubbing rules: kept when its personal data scan passes (201), refused with the scan's counts and no copy when it does not (409)").status(201).errors(&[400, 404, 409, 422, 502]))
        .get("/v1/test-lab/runs/{id}/scrub", list_route)
        .doc(RouteDoc::new("The scrubbed copies a run asked for: their outcomes and what their personal data scans found (counts, never values)").errors(&[404]))
}

/// The body of `POST /v1/test-lab/runs/{id}/scrub`.
#[derive(Debug, Deserialize)]
struct ScrubBody {
    source: String,
}

fn scrub_route(app: &App, req: &Request, p: &Params<'_>) -> Response {
    let body: ScrubBody = match crate::nodes::parse(req) {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    match copy_for_run(app, p.get("id"), &body.source) {
        Ok(view) => {
            let status = if view["state"] == "copied" { 201 } else { 409 };
            Response::json(status, &view)
        }
        Err((status, message)) => Response::error(status, &message),
    }
}

fn list_route(app: &App, _req: &Request, p: &Params<'_>) -> Response {
    let id = p.get("id");
    if app.test_lab.runs().iter().all(|run| run.id != id) {
        return Response::error(404, "no such run");
    }
    let (scrubs, findings) = {
        let inner = app.test_lab.inner.lock().unwrap_or_else(|e| e.into_inner());
        let scrubs: Vec<LabScrub> = inner
            .scrubs
            .iter()
            .filter(|s| s.run == id)
            .cloned()
            .collect();
        let findings: Vec<LabScrubFinding> = inner
            .scrub_findings
            .iter()
            .filter(|f| f.run == id)
            .cloned()
            .collect();
        (scrubs, findings)
    };
    let copies: Vec<Value> = scrubs
        .iter()
        .map(|s| {
            let found: Vec<&LabScrubFinding> =
                findings.iter().filter(|f| f.scrub == s.seq).collect();
            json!({"copy": s, "findings": found})
        })
        .collect();
    Response::json(200, &json!({"run": id, "copies": copies}))
}

/// Copies `source` for run `run_id` and records the outcome. The view is the copy's record and its scan's report; a
/// refused copy is recorded too, with no file kept. Errors are the status and a message that names no value.
pub fn copy_for_run(app: &App, run_id: &str, source: &str) -> Result<Value, (u16, String)> {
    let run = app
        .test_lab
        .runs()
        .into_iter()
        .find(|run| run.id == run_id)
        .ok_or((404, "no such run".to_string()))?;
    if run.state == "done" {
        return Err((409, "that run is done".into()));
    }
    if !valid_label(source) {
        return Err((400, "the source is a label".into()));
    }
    let rules = match crate::topology::read_file(app, &run.project, scrub::RULES_FILE) {
        Ok(bytes) => {
            let text = String::from_utf8(bytes)
                .map_err(|_| (422, format!("{} is not UTF-8", scrub::RULES_FILE)))?;
            Rules::parse(&text).map_err(|why| (422, why))?
        }
        // A project with no rules file has no rules: the scan then decides.
        Err((404, _)) => Rules::default(),
        Err(refused) => return Err(refused),
    };
    let binding = format!("connector:{source}");
    let url = app
        .vault
        .value_for(URL_NAME, &binding, &OnBehalf::Hub)
        .ok_or((422, "the source has no URL in the vault".to_string()))?;
    let ca = app.vault.value_for(CA_NAME, &binding, &OnBehalf::Hub);
    let salt = crate::vault::random_salt().map_err(|why| (503, why))?;
    let mut scrubber = Scrubber::new(rules, salt).map_err(|why| (422, why))?;
    let dir = run_dir(app, run_id).join("scrub");

    let mut db: Box<dyn Source> = match url.strip_prefix(SQLITE_PREFIX) {
        Some(path) => Box::new(Sqlite::open(Path::new(path)).map_err(|why| (502, why))?),
        None => Box::new(Postgres::connect(&url, ca.as_deref()).map_err(|why| (502, why))?),
    };
    let scrub_seq = {
        let inner = app.test_lab.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.scrubs.iter().filter(|s| s.run == run_id).count() as u64 + 1
    };
    let now = apz_plaza::now_ms();
    let outcome = scrub::copy(db.as_mut(), &mut scrubber, &dir);
    let (state, report, reason) = match outcome {
        Ok(copy) => {
            let report = copy.report.clone();
            // The file stays for the run; `discard` removes it when the run ends.
            let _ = copy.keep();
            ("copied", report, String::new())
        }
        Err(Refusal::Scan(report)) => ("refused", report, String::new()),
        Err(Refusal::Failed(why)) => ("failed", scrub::Report::default(), why),
    };
    {
        let mut inner = app.test_lab.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.scrubs.push(LabScrub {
            run: run_id.to_string(),
            seq: scrub_seq,
            source: source.to_string(),
            state: state.to_string(),
            tables: report.tables,
            cells: report.cells,
            cut_documents: report.cut_documents,
            reason: reason.clone(),
            at_ms: now,
        });
        for finding in &report.findings {
            inner.scrub_findings.push(LabScrubFinding {
                run: run_id.to_string(),
                scrub: scrub_seq,
                table: finding.table.clone(),
                column: finding.column.clone(),
                kind: finding.kind.clone(),
                samples: finding.samples,
            });
        }
    }
    if state == "failed" {
        return Err((502, reason));
    }
    Ok(json!({
        "run": run_id,
        "source": source,
        "state": state,
        "report": report,
    }))
}

/// A source's label: lowercase letters, digits and dashes, as a vault connector id is.
fn valid_label(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 63
        && !text.starts_with('-')
        && !text.ends_with('-')
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
