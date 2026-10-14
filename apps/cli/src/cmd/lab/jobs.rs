//! `agentplaza lab jobs`: the training jobs. A job file is the job document the New job form sends:
//! `recipe`, `datasets`, `budget_gpu_hours`, and the optional `devices`, `mode`, `train`, `search`.

use std::time::Duration;

use clap::Subcommand;
use serde_json::Value;

use super::{amount, job_path, refusal};
use crate::cmd::models::{items, joined, none_if_empty};
use crate::ctx::{self, confirm, Ctx};

const STATUSES: [&str; 5] = ["queued", "running", "done", "failed", "cancelled"];
const MODES: [&str; 3] = ["single", "tight", "loose"];
/// Seconds between the polls of `--follow`.
const FOLLOW_EVERY: Duration = Duration::from_secs(2);

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The jobs, newest first, with their status, recipe and GPU-hours. The filters are applied here.
    List {
        /// Only the jobs in this status.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(STATUSES))]
        status: Option<String>,
        /// Only the jobs of this recipe: name@version, or the name alone for any version.
        #[arg(long)]
        recipe: Option<String>,
        /// Only this mode: single, tight or loose.
        #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(MODES))]
        mode: Option<String>,
    },
    /// One job in full: its plan, its runs and its outputs.
    Show {
        /// The job's id.
        id: String,
    },
    /// The job's events, its trainer's own lines. With --follow, keeps polling every 2 s until the job is over.
    Events {
        /// The job's id.
        id: String,
        /// Only the events after this sequence number.
        #[arg(long, default_value_t = 0)]
        after: u64,
        /// Keep polling until the job ends.
        #[arg(long)]
        follow: bool,
    },
    /// The plan a job file would get. A dry run: nothing is written or dispatched.
    Plan {
        /// The job file (JSON).
        file: String,
    },
    /// Submit a job file: it is planned, its directory written, and it is dispatched to the machines.
    Submit {
        /// The job file (JSON).
        file: String,
    },
    /// Cancel a job. Asks first unless --yes.
    Cancel {
        /// The job's id.
        id: String,
        /// Cancel without asking.
        #[arg(long)]
        yes: bool,
    },
}

pub fn run(ctx: &Ctx, cmd: Cmd) -> Result<(), String> {
    match cmd {
        Cmd::List { status, recipe, mode } => list(ctx, status.as_deref(), recipe.as_deref(), mode.as_deref()),
        Cmd::Show { id } => {
            let job = ctx.client.get(&job_path(&id))?;
            ctx.out(&job, print_job);
            Ok(())
        }
        Cmd::Events { id, after, follow } => events(ctx, &id, after, follow),
        Cmd::Plan { file } => {
            let doc = read_job(&file)?;
            let plan = ctx.client.post("/v1/foundry/plan", &doc)?;
            ctx.out(&plan, print_plan);
            Ok(())
        }
        Cmd::Submit { file } => submit(ctx, &file),
        Cmd::Cancel { id, yes } => cancel(ctx, &id, yes),
    }
}

/// Whether a job summary passes the filters: its status, its mode, and its recipe (exact, or the name alone).
pub fn keep(job: &Value, status: Option<&str>, recipe: Option<&str>, mode: Option<&str>) -> bool {
    let recipe_ok = recipe.is_none_or(|want| {
        let got = ctx::s(job, "recipe");
        got == want || got.split_once('@').map_or(got, |(name, _)| name) == want
    });
    status.is_none_or(|s| ctx::s(job, "status") == s) && mode.is_none_or(|m| ctx::s(job, "mode") == m) && recipe_ok
}

/// The job list, filtered here: the hub's list takes no filters.
fn list(ctx: &Ctx, status: Option<&str>, recipe: Option<&str>, mode: Option<&str>) -> Result<(), String> {
    let all = ctx.client.get("/v1/foundry/jobs")?;
    let kept = Value::Array(items(&all).filter(|j| keep(j, status, recipe, mode)).cloned().collect());
    ctx.out(&kept, |v| {
        let rows: Vec<Vec<String>> = items(v)
            .map(|j| {
                vec![
                    ctx::s(j, "id").to_string(),
                    ctx::s(j, "status").to_string(),
                    ctx::s(j, "mode").to_string(),
                    ctx::s(j, "recipe").to_string(),
                    amount(&j["gpu_hours"]),
                    amount(&j["budget_gpu_hours"]),
                    ctx::text(j, "runs"),
                    ctx::when(&j["created_ms"]),
                ]
            })
            .collect();
        ctx::table(&["job", "status", "mode", "recipe", "gpu-h", "budget", "runs", "created"], &rows);
    });
    Ok(())
}

/// A job file, read whole and parsed.
fn read_job(file: &str) -> Result<Value, String> {
    let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{file}: not JSON: {e}"))
}

/// The job's status is read first: a job that is over has written all its events, so an empty page
/// read after that is the last one.
fn events(ctx: &Ctx, id: &str, after: u64, follow: bool) -> Result<(), String> {
    let path = job_path(id);
    if !follow {
        let page = ctx.client.get(&format!("{path}/events?after={after}"))?;
        ctx.out(&page, |p| items(&p["events"]).for_each(|e| println!("{}", event_line(e))));
        return Ok(());
    }
    let mut cursor = after;
    loop {
        let over = is_over(&status_of(ctx, id)?);
        let page = ctx.client.get(&format!("{path}/events?after={cursor}"))?;
        let rows: Vec<&Value> = items(&page["events"]).collect();
        for e in &rows {
            if ctx.json {
                println!("{e}");
            } else {
                println!("{}", event_line(e));
            }
        }
        cursor = cursor.max(page["next"].as_u64().unwrap_or(cursor));
        if over && rows.is_empty() {
            return Ok(());
        }
        std::thread::sleep(FOLLOW_EVERY);
    }
}

/// A job's status from the list (the summary, without the series that the job view carries).
fn status_of(ctx: &Ctx, id: &str) -> Result<Value, String> {
    let all = ctx.client.get("/v1/foundry/jobs")?;
    items(&all)
        .find(|j| ctx::s(j, "id") == id)
        .cloned()
        .ok_or_else(|| format!("no job {id}"))
}

/// Whether a job has stopped: done, failed or cancelled.
fn is_over(job: &Value) -> bool {
    matches!(ctx::s(job, "status"), "done" | "failed" | "cancelled")
}

/// One event as a line: its sequence number, its name, its island and node, and the rest of its fields.
pub fn event_line(e: &Value) -> String {
    let ev = &e["event"];
    let name = ctx::s(ev, "ev");
    let details = match ev {
        Value::Object(map) => {
            let mut rest = map.clone();
            rest.remove("ev");
            Value::Object(rest).to_string()
        }
        other => other.to_string(),
    };
    format!("{:>6}  {:<20} {:<10} {:<12} {}", ctx::text(e, "seq"), name, ctx::s(e, "island"), ctx::s(e, "node"), details)
}

/// Submits the job. A 2xx answer is the job; a refusal is its `error`. A 422 that carries the job means
/// the job was recorded and its dispatch failed: the job's id goes with the reason.
fn submit(ctx: &Ctx, file: &str) -> Result<(), String> {
    let doc = read_job(file)?;
    let path = "/v1/foundry/jobs";
    let (status, body) = apz_http::client::request_json(&ctx.client.target, "POST", path, Some(&doc), ctx.client.timeout)
        .map_err(|e| format!("{} {path}: {e}", ctx.client.base()))?;
    if (200..300).contains(&status) {
        ctx.out(&body, |j| println!("job {} is {}", ctx::s(j, "id"), ctx::s(j, "status")));
        return Ok(());
    }
    ctx.out(&body, |_| {});
    let reason = refusal(&body, path, status);
    match body.get("job") {
        Some(job) => Err(format!("job {} was recorded but did not dispatch: {reason}", ctx::s(job, "id"))),
        None => Err(reason),
    }
}

fn cancel(ctx: &Ctx, id: &str, yes: bool) -> Result<(), String> {
    if !confirm(yes, &format!("Cancel job {id}."))? {
        return Err("cancelled".into());
    }
    let job = ctx.client.call("POST", &format!("{}/cancel", job_path(id)), None)?;
    ctx.out(&job, |j| println!("job {} is {}", ctx::s(j, "id"), ctx::s(j, "status")));
    Ok(())
}

/// A plan: the mode, the islands and their machines, the devices and steps, and where the output goes.
fn print_plan(p: &Value) {
    ctx::fields(&[
        ("mode", ctx::s(p, "mode").to_string()),
        ("devices", ctx::text(p, "devices_total")),
        ("islands", items(&p["islands"]).count().to_string()),
        ("inner steps", ctx::text(p, "inner_steps")),
        ("staleness", ctx::text(p, "staleness")),
        ("rounds", none_if_empty(ctx::text(p, "rounds"))),
        ("frozen", none_if_empty(ctx::text(p, "frozen"))),
        ("output", ctx::s(p, "output_dir").to_string()),
    ]);
    let rows: Vec<Vec<String>> = items(&p["islands"])
        .map(|i| {
            let machines: Vec<&str> = items(&i["machines"]).map(|m| ctx::s(m, "id")).collect();
            vec![ctx::s(i, "id").to_string(), ctx::s(i, "backend").to_string(), ctx::s(i, "leader").to_string(), machines.join(", ")]
        })
        .collect();
    ctx::table(&["island", "backend", "leader", "machines"], &rows);
}

/// A job in full: its summary, its plan's outputs, and its runs.
fn print_job(j: &Value) {
    ctx::fields(&[
        ("id", ctx::s(j, "id").to_string()),
        ("status", ctx::s(j, "status").to_string()),
        ("mode", ctx::s(j, "mode").to_string()),
        ("recipe", ctx::s(j, "recipe").to_string()),
        ("datasets", none_if_empty(joined(j.get("datasets")))),
        ("gpu-h", format!("{} of {} budget", amount(&j["gpu_hours"]), amount(&j["budget_gpu_hours"]))),
        ("created", ctx::when(&j["created_ms"])),
        ("updated", ctx::when(&j["updated_ms"])),
        ("error", none_if_empty(ctx::text(j, "error"))),
        ("outputs", none_if_empty(joined(j.get("outputs")))),
    ]);
    let rows: Vec<Vec<String>> = items(&j["runs"])
        .map(|r| {
            vec![
                ctx::s(r, "node").to_string(),
                ctx::s(r, "island").to_string(),
                items(&r["devices"]).map(|d| d.to_string()).collect::<Vec<_>>().join(","),
                ctx::s(r, "state").to_string(),
                ctx::text(r, "exit_code"),
                amount(&r["gpu_hours"]),
                ctx::when(&r["started_ms"]),
            ]
        })
        .collect();
    println!();
    ctx::table(&["node", "island", "devices", "state", "exit", "gpu-h", "started"], &rows);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn job(status: &str, mode: &str, recipe: &str) -> Value {
        json!({ "id": "j", "status": status, "mode": mode, "recipe": recipe })
    }

    #[test]
    fn the_filters_pick_by_status_mode_and_recipe_name_or_version() {
        let j = job("running", "tight", "cls@1.2");
        assert!(keep(&j, None, None, None));
        assert!(keep(&j, Some("running"), None, None));
        assert!(!keep(&j, Some("done"), None, None));
        assert!(keep(&j, None, None, Some("tight")));
        assert!(!keep(&j, None, None, Some("loose")));
        assert!(keep(&j, None, Some("cls@1.2"), None), "an exact recipe");
        assert!(keep(&j, None, Some("cls"), None), "the name alone matches any version");
        assert!(!keep(&j, None, Some("cls@1.3"), None));
        assert!(!keep(&j, None, Some("c"), None), "a name prefix is not the name");
    }

    #[test]
    fn an_event_line_names_its_step_and_keeps_its_fields() {
        let e = json!({ "seq": 3, "island": "i0", "node": "n1", "event": { "ev": "step", "loss": 0.5 } });
        let line = event_line(&e);
        assert!(line.contains("step"), "{line}");
        assert!(line.contains("\"loss\":0.5"), "{line}");
        assert!(!line.contains("\"ev\""), "the name is shown once: {line}");
        assert!(line.trim_start().starts_with('3'), "{line}");
    }

    #[test]
    fn a_job_is_over_only_when_it_is_done_failed_or_cancelled() {
        assert!(is_over(&job("done", "single", "r@1")));
        assert!(is_over(&job("failed", "single", "r@1")));
        assert!(is_over(&job("cancelled", "single", "r@1")));
        assert!(!is_over(&job("running", "single", "r@1")));
        assert!(!is_over(&job("queued", "single", "r@1")));
    }
}
