//! The Foundry's HTTP routes, the device view, and the tracker thread that polls the runs.

use std::sync::Arc;
use std::time::Duration;

use apz_http::{Params, Request, Response, RouteDoc, Router};
use serde_json::{Value, json};

use crate::fleet;
use crate::foundry::datasets::{self, ExportSpec};
use crate::foundry::diloco::{Config as HubConfig, Hub};
use crate::foundry::jobs::{self, NodeCalls};
use crate::foundry::plan::Mode;
use crate::foundry::plan::{Accelerator, Machine, Reach};
use crate::foundry::store::Job;
use crate::foundry::wire;
use crate::state::App;

/// A machine whose heartbeat is older than this is offline (the fleet's own rule).
const OFFLINE_AFTER_MS: u64 = crate::nodes::OFFLINE_AFTER_MS;

/// Calls to noded through the fleet: the local noded, or a joined node's command queue.
pub struct Fleet<'a>(pub &'a App);

impl NodeCalls for Fleet<'_> {
    fn call(
        &self,
        node: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Value), String> {
        fleet::noded(self.0, node, method, path, body)
    }
}

pub fn routes(router: Router<App>) -> Router<App> {
    let router = super::hf_import::routes(router);
    router
        .get("/v1/foundry/recipes", |app, _, _| {
            Response::json(
                200,
                &app.foundry
                    .read(|s| s.recipes.values().cloned().collect::<Vec<_>>()),
            )
        })
        .doc(RouteDoc::new("List the recipes"))
        .post("/v1/foundry/recipes/{id}/vet", |app, req, p| {
            vet(app, p.get("id"), &req.body)
        })
        .doc(RouteDoc::new("Vet a recipe's commit").errors(&[400, 403, 404]))
        .get("/v1/foundry/datasets", |app, _, _| {
            Response::json(
                200,
                &app.foundry
                    .read(|s| s.datasets.values().cloned().collect::<Vec<_>>()),
            )
        })
        .doc(RouteDoc::new("List the datasets"))
        .post("/v1/foundry/datasets", |app, req, _| export(app, &req.body))
        .doc(
            RouteDoc::new("Export a dataset from the plaza's records")
                .status(201)
                .errors(&[400]),
        )
        .post("/v1/foundry/datasets/upload", |app, req, _| {
            upload(app, req)
        })
        .doc(RouteDoc::new("Upload a dataset").status(201).errors(&[400]))
        .get("/v1/foundry/devices", |app, _, _| {
            Response::json(200, &devices_view(app))
        })
        .doc(RouteDoc::new(
            "List the accelerators and the jobs holding them",
        ))
        .post("/v1/foundry/plan", |app, req, _| dry_run(app, &req.body))
        .doc(RouteDoc::new("Plan a job without starting it").errors(&[400]))
        .post("/v1/foundry/jobs", |app, req, _| submit(app, &req.body))
        .doc(
            RouteDoc::new("Submit a training job")
                .status(201)
                .errors(&[400, 409, 422]),
        )
        .get("/v1/foundry/jobs", |app, _, _| {
            Response::json(
                200,
                &job_list(
                    &app.foundry
                        .read(|s| s.jobs.values().cloned().collect::<Vec<Job>>()),
                ),
            )
        })
        .doc(RouteDoc::new("List the jobs, newest first"))
        .get("/v1/foundry/jobs/{id}", |app, _, p| {
            match app.foundry.read(|s| s.jobs.get(p.get("id")).cloned()) {
                Some(job) => Response::json(200, &job_view(&job)),
                None => Response::error(404, "no such job"),
            }
        })
        .doc(RouteDoc::new("Get one job").errors(&[404]))
        .get("/v1/foundry/jobs/{id}/events", |app, req, p| {
            events(app, req, p)
        })
        .doc(RouteDoc::new("List a job's events after a sequence number").errors(&[404]))
        .post("/v1/foundry/jobs/{id}/hub/init", |app, req, p| {
            hub_call(app, req, p, HubCall::Init)
        })
        .doc(RouteDoc::new("Initialize a loose job's hub").errors(&[400, 403, 404]))
        .get("/v1/foundry/jobs/{id}/hub/state", |app, req, p| {
            hub_call(app, req, p, HubCall::State)
        })
        .doc(RouteDoc::new("Read a loose job's hub state").errors(&[403, 404]))
        .post("/v1/foundry/jobs/{id}/hub/round", |app, req, p| {
            hub_call(app, req, p, HubCall::Round)
        })
        .doc(RouteDoc::new("Submit an island's round to the hub").errors(&[400, 403, 404, 409]))
        .get("/v1/foundry/jobs/{id}/hub/health", |app, req, p| {
            hub_call(app, req, p, HubCall::Health)
        })
        .doc(RouteDoc::new("Check a loose job's hub").errors(&[403, 404]))
        .post(
            "/v1/foundry/jobs/{id}/cancel",
            |app, _, p| match jobs::cancel(
                &Fleet(app),
                &app.foundry,
                p.get("id"),
                apz_plaza::now_ms(),
            ) {
                Some(job) => Response::json(200, &job_view(&job)),
                None => Response::error(404, "no such job"),
            },
        )
        .doc(RouteDoc::new("Cancel a job").errors(&[404]))
}

/// `POST /v1/foundry/recipes/{id}/vet` with `{commit}`: the owner vets a commit of a recipe.
fn vet(app: &App, id: &str, body: &[u8]) -> Response {
    let commit = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v["commit"].as_str().map(str::to_string))
        .unwrap_or_default();
    if !valid_commit(&commit) {
        return Response::error(400, "commit is 7 to 64 hex characters");
    }
    let mut state = app.foundry.write();
    let Some(recipe) = state.recipes.get_mut(id) else {
        return Response::error(404, &format!("no recipe {id}"));
    };
    if recipe.local_dev && recipe.commit != commit {
        return Response::error(403, "a local dev recipe is vetted only at its own pin");
    }
    recipe.commit = commit;
    recipe.vetted_by = Some("owner".into());
    recipe.vetted_ms = Some(apz_plaza::now_ms());
    Response::json(200, &*recipe)
}

fn valid_commit(commit: &str) -> bool {
    ((7..=64).contains(&commit.len()) && commit.chars().all(|c| c.is_ascii_hexdigit()))
        || (commit.starts_with("dev-") && commit.len() > 4)
}

/// `POST /v1/foundry/datasets` with an export spec.
fn export(app: &App, body: &[u8]) -> Response {
    let spec: ExportSpec = match serde_json::from_slice(body) {
        Ok(spec) => spec,
        Err(err) => return Response::error(400, &format!("export spec: {err}")),
    };
    match export_dataset(app, &spec) {
        Ok(ds) => Response::json(201, &ds),
        Err(err) => Response::error(400, &err),
    }
}

/// Exports a dataset from the plaza's records and records it in the Foundry. The route and the
/// breeder's `foundry_export` tool both run it.
pub(crate) fn export_dataset(app: &App, spec: &ExportSpec) -> Result<datasets::Dataset, String> {
    let ds = datasets::export(app, &app.foundry.dir, spec)?;
    app.foundry
        .write()
        .datasets
        .insert(ds.id.clone(), ds.clone());
    Ok(ds)
}

/// `POST /v1/foundry/datasets/upload?name=N` (JSONL body), or `{name, path}` JSON naming a file.
fn upload(app: &App, req: &Request) -> Response {
    let now = apz_plaza::now_ms();
    let result = match req.query_param("name") {
        Some(name) => datasets::upload(&app.foundry.dir, name, &req.body, now),
        None => {
            let Ok(doc) = serde_json::from_slice::<Value>(&req.body) else {
                return Response::error(400, "send JSONL with ?name=, or {name, path}");
            };
            let (Some(name), Some(path)) = (doc["name"].as_str(), doc["path"].as_str()) else {
                return Response::error(400, "{name, path} names the upload");
            };
            if !std::path::Path::new(path).is_absolute() {
                return Response::error(400, "path is absolute");
            }
            match std::fs::metadata(path) {
                Ok(meta) if meta.len() <= 50 << 20 => match std::fs::read(path) {
                    Ok(bytes) => datasets::upload(&app.foundry.dir, name, &bytes, now),
                    Err(err) => Err(format!("read {path}: {err}")),
                },
                Ok(_) => Err("the file is over 50 MB".into()),
                Err(err) => Err(format!("{path}: {err}")),
            }
        }
    };
    match result {
        Ok(ds) => {
            app.foundry
                .write()
                .datasets
                .insert(ds.id.clone(), ds.clone());
            Response::json(201, &ds)
        }
        Err(err) => Response::error(400, &err),
    }
}

/// The fleet's machines for the planner, and the round trips between them. Round trips are not
/// measured across machines yet, so each machine is its own island.
pub fn machines(app: &App) -> (Vec<Machine>, Reach) {
    let mut all = Vec::new();
    if let Ok((200, facts)) = app.noded.get("/v1/machine") {
        let facts: crate::nodes::Machine = serde_json::from_value(facts).unwrap_or_default();
        all.push(machine_of(fleet::LOCAL, &facts, true));
    }
    let now = apz_plaza::now_ms();
    for node in app.nodes.list() {
        if node.revoked {
            continue;
        }
        let online = now.saturating_sub(node.last_seen_ms) < OFFLINE_AFTER_MS;
        all.push(machine_of(&node.id, &node.machine, online));
    }
    (all, Reach::default())
}

/// A planner machine from noded's facts (`accelerators`, `trainer`, `fabric`).
fn machine_of(id: &str, facts: &crate::nodes::Machine, online: bool) -> Machine {
    Machine {
        id: id.into(),
        online,
        accelerators: planner_accelerators(&facts.accelerators),
        trainer_ok: facts.trainer.as_ref().is_some_and(|trainer| trainer.ok),
        fabric: facts.fabric.clone(),
    }
}

/// The planner's accelerators from a node's: a device that lacks a figure the planner reads makes the whole list
/// empty, as the planner has always read it.
fn planner_accelerators(devices: &[crate::nodes::MachineAccelerator]) -> Vec<Accelerator> {
    devices
        .iter()
        .map(|device| {
            Some(Accelerator {
                index: device.index,
                vendor: device.vendor.clone(),
                name: device.name.clone(),
                memory_mib: device.memory_mib?,
                backend: device.backend.clone(),
                link: device.link.clone(),
                busy: device.busy,
                util_pct: device.util_pct?,
                mem_used_mib: device.mem_used_mib?,
            })
        })
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default()
}

/// `GET /v1/foundry/devices`: every accelerator, with the job whose running run holds it.
pub fn devices_view(app: &App) -> Value {
    let (machines, _) = machines(app);
    let allocated = app.foundry.read(|s| {
        let mut held: Vec<(String, u32, String)> = Vec::new();
        for job in s.jobs.values().filter(|j| j.status == "running") {
            for run in job.runs.iter().filter(|r| r.state == "running") {
                for device in &run.devices {
                    held.push((run.node.clone(), *device, job.id.clone()));
                }
            }
        }
        held
    });
    let view: Vec<Value> = machines
        .iter()
        .map(|m| {
            let devices: Vec<Value> = m
                .accelerators
                .iter()
                .map(|a: &Accelerator| {
                    let by = allocated.iter().find(|(node, index, _)| node == &m.id && *index == a.index).map(|(_, _, job)| job.clone());
                    json!({"index": a.index, "vendor": a.vendor, "name": a.name, "backend": a.backend, "memory_mib": a.memory_mib,
                           "mem_used_mib": a.mem_used_mib, "busy": a.busy, "util_pct": a.util_pct, "allocated_to": by})
                })
                .collect();
            json!({"id": m.id, "online": m.online, "trainer_ok": m.trainer_ok, "devices": devices})
        })
        .collect();
    json!({"machines": view})
}

/// `POST /v1/foundry/plan`: the dry run. Nothing is written or dispatched.
fn dry_run(app: &App, body: &[u8]) -> Response {
    let doc: Value = match serde_json::from_slice(body) {
        Ok(doc) => doc,
        Err(err) => return Response::error(400, &format!("job: {err}")),
    };
    let spec = match jobs::parse(&doc, jobs::new_job_id) {
        Ok(spec) => spec,
        Err((status, msg)) => return Response::error(status, &msg),
    };
    let result = jobs::budget_check(&spec)
        .and_then(|()| jobs::law_check(app, &spec))
        .and_then(|()| jobs::admit(&app.foundry, &spec))
        .and_then(|found| {
            let (machines, reach) = machines(app);
            let plan = jobs::place(&machines, &reach, &spec)?;
            Ok(jobs::plan_document(&plan, &spec, &found))
        });
    match result {
        Ok(plan) => Response::json(200, &plan),
        Err((status, msg)) => Response::error(status, &msg),
    }
}

/// `POST /v1/foundry/jobs`: the job document is parsed here; [`submit_job`] does the rest.
fn submit(app: &App, body: &[u8]) -> Response {
    let doc: Value = match serde_json::from_slice(body) {
        Ok(doc) => doc,
        Err(err) => return Response::error(400, &format!("job: {err}")),
    };
    match submit_job(app, doc) {
        Ok(job) => {
            let view = job_view(&job);
            match job.error {
                None => Response::json(201, &view),
                Some(msg) => Response::json(422, &json!({"error": msg, "job": view})),
            }
        }
        Err((status, msg)) => Response::error(status, &msg),
    }
}

/// Submits a job document: the parse, the checks (budget, laws, recipe and datasets), the plan, the
/// job directory, the hub of a loose job, then the dispatch to the machines. The job is recorded
/// either way: a dispatch failure stays on it (`status` "failed", `error`), so `Ok` carries that job.
/// A refusal is its HTTP status and message. The route and the breeder's `foundry_submit_job` tool
/// both run it.
pub(crate) fn submit_job(app: &App, doc: Value) -> Result<Job, (u16, String)> {
    let spec = jobs::parse(&doc, jobs::new_job_id)?;
    if app.foundry.read(|s| s.jobs.contains_key(&spec.job_id)) {
        return Err((409, format!("job {} exists", spec.job_id)));
    }
    let now = apz_plaza::now_ms();
    let prepared = jobs::budget_check(&spec)
        .and_then(|()| jobs::law_check(app, &spec))
        .and_then(|()| jobs::admit(&app.foundry, &spec))
        .and_then(|found| {
            let (machines, reach) = machines(app);
            let plan = jobs::place(&machines, &reach, &spec)?;
            let plan_doc = jobs::plan_document(&plan, &spec, &found);
            let dir = jobs::write_job_dir(&app.foundry, &spec.job_id, &doc, &plan_doc, &found)?;
            Ok((plan, plan_doc, dir))
        });
    let (plan, plan_doc, dir) = prepared?;
    let hub_base = format!("http://127.0.0.1:{}", app.config.port);
    if plan.mode == Mode::Loose {
        let islands: Vec<String> = plan.islands.iter().map(|i| i.id.clone()).collect();
        let cfg = HubConfig {
            staleness: u64::from(plan.staleness),
            ..HubConfig::default()
        };
        app.foundry
            .hubs
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(spec.job_id.clone(), Arc::new(Hub::new(&islands, cfg)));
    }
    let dispatched = jobs::dispatch(
        &Fleet(app),
        &spec.job_id,
        &plan,
        &dir,
        &app.foundry.recipes_dir(),
        &hub_base,
        now,
    );
    let (status, error, runs) = match dispatched {
        Ok(runs) => ("running".to_string(), None, runs),
        Err((_, msg)) => ("failed".to_string(), Some(msg), Vec::new()),
    };
    let job = Job {
        candidates: Vec::new(),
        id: spec.job_id.clone(),
        status,
        created_ms: now,
        updated_ms: now,
        recipe: spec.recipe.clone(),
        datasets: spec.datasets.clone(),
        mode: plan.mode.to_string(),
        budget_gpu_hours: spec.budget,
        gpu_hours: 0.0,
        doc,
        plan: plan_doc,
        runs,
        series: Vec::new(),
        seq: 0,
        outputs: Vec::new(),
        outputs_remote: false,
        metrics: Value::Null,
        error,
    };
    app.foundry.write().jobs.insert(job.id.clone(), job.clone());
    Ok(job)
}

/// The job list, newest first, without the series (that is in the job view).
fn job_list(jobs: &[Job]) -> Value {
    let mut rows: Vec<&Job> = jobs.iter().collect();
    rows.sort_by(|a, b| {
        b.created_ms
            .cmp(&a.created_ms)
            .then_with(|| b.id.cmp(&a.id))
    });
    Value::Array(rows.into_iter().map(job_summary).collect())
}

fn job_summary(job: &Job) -> Value {
    json!({"id": job.id, "status": job.status, "mode": job.mode, "recipe": job.recipe, "datasets": job.datasets,
           "created_ms": job.created_ms, "updated_ms": job.updated_ms, "gpu_hours": job.gpu_hours,
           "budget_gpu_hours": job.budget_gpu_hours, "error": job.error, "runs": job.runs.len(),
           // The breeder (agent cid) that submitted it, if an agent did; the owner's own jobs have none.
           "agent": job.doc.get("agent").and_then(Value::as_u64)})
}

/// One job in full: the summary, its plan, runs, series, metrics and outputs.
pub fn job_view(job: &Job) -> Value {
    let mut view = job_summary(job);
    view["plan"] = job.plan.clone();
    view["runs"] = json!(job.runs);
    view["series"] = json!(job.series);
    view["metrics"] = job.metrics.clone();
    view["outputs"] = json!(job.outputs);
    view["outputs_remote"] = json!(job.outputs_remote);
    view["seq"] = json!(job.seq);
    view
}

/// `GET /v1/foundry/jobs/{id}/events?after=SEQ`: the job's events after a sequence number, from
/// its `events.jsonl`, with `next` for the following call.
fn events(app: &App, req: &Request, p: &Params) -> Response {
    let id = p.get("id");
    if app.foundry.read(|s| !s.jobs.contains_key(id)) {
        return Response::error(404, "no such job");
    }
    let after = req
        .query_param("after")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let text = std::fs::read_to_string(app.foundry.jobs_dir().join(id).join("events.jsonl"))
        .unwrap_or_default();
    let mut next = after;
    let mut events = Vec::new();
    for line in text.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let seq = event["seq"].as_u64().unwrap_or(0);
        next = next.max(seq);
        if seq > after {
            events.push(event);
        }
    }
    Response::json(200, &json!({"job": id, "events": events, "next": next}))
}

/// The tracker: every two seconds, poll each running job's runs, publish the events on the bus
/// (`training`) and keep the series. Runs for the life of the process.
pub fn spawn_tracker(app: Arc<App>) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let running: Vec<String> = app.foundry.read(|s| {
                s.jobs
                    .values()
                    .filter(|j| j.status == "running")
                    .map(|j| j.id.clone())
                    .collect()
            });
            for id in running {
                for line in jobs::poll(
                    &Fleet(&app),
                    &app.foundry,
                    &app.models,
                    &id,
                    apz_plaza::now_ms(),
                ) {
                    app.bus
                        .publish("training", &json!({"job": id, "line": line}));
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commits_are_hex_or_a_dev_pin() {
        assert!(valid_commit("abc1234"));
        assert!(valid_commit("dev-0123456789abcdef"));
        assert!(!valid_commit("zz"));
        assert!(!valid_commit("abc12"));
        assert!(!valid_commit("nothex-nothex"));
    }

    #[test]
    fn the_job_list_is_newest_first_without_the_series() {
        let mut older = Job {
            id: "a".into(),
            created_ms: 1,
            ..Job::default()
        };
        older.series = vec![json!({"step": 1})];
        let newer = Job {
            id: "b".into(),
            created_ms: 2,
            ..Job::default()
        };
        let list = job_list(&[older, newer]);
        assert_eq!(list[0]["id"], "b");
        assert!(list[1].get("series").is_none());
    }
}

/// The DiLoCo hub's routes, one per call an island makes (PROTOCOL.md section 1).
#[derive(Clone, Copy)]
enum HubCall {
    Init,
    State,
    Round,
    Health,
}

/// The hub's calls from this machine's islands only: the trainer cannot send credentials, so the
/// loopback peer is the trust boundary (remote loose islands are not wired yet).
fn hub_call(app: &App, req: &Request, p: &Params, call: HubCall) -> Response {
    if !req.peer.is_some_and(|peer| peer.is_loopback()) {
        return Response::error(403, "the hub takes calls from its own machine only");
    }
    let id = p.get("id");
    let Some(hub) = app
        .foundry
        .hubs
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .cloned()
    else {
        return Response::error(404, "no loose hub for this job");
    };
    let binary = |status: u16, meta: &Value, tensors: &wire::Tensors| {
        Response::new(
            status,
            "application/octet-stream",
            wire::pack(meta, tensors),
        )
    };
    match call {
        HubCall::Health => Response::json(200, &json!({"ok": true})),
        HubCall::State => {
            let (round, weights) = hub.state();
            binary(200, &json!({"round": round}), &weights)
        }
        HubCall::Init => {
            let (meta, tensors) = match wire::unpack(&req.body) {
                Ok(parsed) => parsed,
                Err(err) => return Response::error(400, &err),
            };
            let (round, weights) = hub.init(tensors);
            let _ = meta;
            binary(200, &json!({"round": round}), &weights)
        }
        HubCall::Round => {
            let (meta, delta) = match wire::unpack(&req.body) {
                Ok(parsed) => parsed,
                Err(err) => return Response::error(400, &err),
            };
            let (Some(island), Some(round), Some(samples)) = (
                meta["island"].as_str(),
                meta["round"].as_u64(),
                meta["samples"].as_f64(),
            ) else {
                return Response::error(400, "meta needs island, round and samples");
            };
            match hub.submit(island, round, samples, delta) {
                Ok(done) => {
                    let status = if done.accepted { 200 } else { 409 };
                    binary(
                        status,
                        &json!({"round": done.round, "accepted": done.accepted}),
                        &done.weights,
                    )
                }
                Err(err) => Response::error(400, &err),
            }
        }
    }
}
