//! Jobs: the job document's checks, the plan document the trainer reads, the dispatch to each
//! machine's noded, and the tracker that polls the runs, keeps the series and meters GPU-hours.
//!
//! Every network call goes through [`NodeCalls`], so the tests drive the flow with a fake noded.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::foundry::datasets::{self, Dataset};
use crate::foundry::plan::{self, Machine, Mode, Plan, Reach, Request};
use crate::foundry::store::{Foundry, Job, Point, Run, SERIES_CAP};

/// The most GPU-hours one job may spend (no grant mechanism yet).
pub const MAX_BUDGET_GPU_HOURS: f64 = 8.0;
/// The image's name prefix; the backend follows it.
const IMAGE: &str = "apz-trainer";
/// The keys a job document may carry (the codex schema, plus the Foundry's extras).
const KEYS: [&str; 10] = [
    "job_id",
    "recipe",
    "datasets",
    "search",
    "budget_gpu_hours",
    "target",
    "export",
    "devices",
    "mode",
    "train",
];

/// A refusal: the HTTP status and the message.
pub type Refused = (u16, String);

/// Calls to a machine's noded. The real one goes through the fleet; tests use a fake.
pub trait NodeCalls {
    fn call(
        &self,
        node: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Value), String>;
}

/// The job document, checked and parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub job_id: String,
    pub recipe: String,
    pub datasets: Vec<String>,
    pub budget: f64,
    pub request: Request,
    pub train: Value,
    pub agent: Option<u64>,
}

/// Checks the document's shape. The laws, the recipe and the datasets are checked by [`admit`].
pub fn parse(doc: &Value, new_id: impl FnOnce() -> String) -> Result<Spec, Refused> {
    let obj = doc
        .as_object()
        .ok_or((400, "the job is a JSON object".to_string()))?;
    if let Some(key) = obj
        .keys()
        .find(|k| !KEYS.contains(&k.as_str()) && k.as_str() != "agent")
    {
        return Err((400, format!("unknown job field {key:?}")));
    }
    let job_id = match obj.get("job_id") {
        None | Some(Value::Null) => new_id(),
        Some(Value::String(id)) => id.clone(),
        Some(_) => return Err((400, "job_id is a string".into())),
    };
    if !valid_id(&job_id) {
        return Err((
            400,
            "job_id is 1-40 letters, digits, '-' or '_', starting with a letter or digit".into(),
        ));
    }
    let recipe = obj
        .get("recipe")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !valid_recipe(&recipe) {
        return Err((400, format!("recipe {recipe:?} is not NAME@VERSION")));
    }
    let datasets: Vec<String> = match obj.get("datasets") {
        Some(Value::Array(items)) if !items.is_empty() => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .ok_or((400, "datasets are strings".to_string()))
            })
            .collect::<Result<_, _>>()?,
        _ => return Err((400, "datasets is a non-empty array of dataset ids".into())),
    };
    let budget = obj
        .get("budget_gpu_hours")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    if budget.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
        return Err((400, "budget_gpu_hours is a number above 0".into()));
    }
    if let Some(export) = obj.get("export")
        && export != "onnx"
    {
        return Err((400, "export is \"onnx\"".into()));
    }
    if let Some(target) = obj.get("target")
        && !matches!(target.as_str(), Some("x86_64" | "aarch64" | "any"))
    {
        return Err((400, "target is x86_64, aarch64 or any".into()));
    }
    let mut request: Request = match obj.get("devices") {
        Some(value) => {
            serde_json::from_value(value.clone()).map_err(|e| (400, format!("devices: {e}")))?
        }
        None => Request::default(),
    };
    if let Some(mode) = obj.get("mode") {
        request.mode = Some(
            serde_json::from_value::<Mode>(mode.clone())
                .map_err(|_| (400, "mode is single, tight or loose".to_string()))?,
        );
    }
    request.budget_gpu_hours = Some(budget);
    if request.min_devices == 0 {
        request.min_devices = 1;
    }
    if request.max_devices == 0 {
        request.max_devices = request.min_devices;
    }
    let train = obj.get("train").cloned().unwrap_or_else(|| json!({}));
    if !train.is_object() {
        return Err((400, "train is an object of overrides".into()));
    }
    let agent = obj.get("agent").and_then(Value::as_u64);
    Ok(Spec {
        job_id,
        recipe,
        datasets,
        budget,
        request,
        train,
        agent,
    })
}

fn valid_id(id: &str) -> bool {
    (1..=40).contains(&id.len())
        && id.starts_with(|c: char| c.is_ascii_alphanumeric())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `NAME@VERSION`: lower-case letters, digits and hyphens, then digits.
fn valid_recipe(recipe: &str) -> bool {
    let Some((name, version)) = recipe.split_once('@') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !version.is_empty()
        && version.chars().all(|c| c.is_ascii_digit())
}

/// A fresh job id: `tj-` and ten hex digits from the clock.
pub fn new_job_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("tj-{:010x}", (nanos as u64) & 0xff_ffff_ffff)
}

/// The checks that need the store: the recipe is vetted, every dataset exists and is clean.
/// Returns the datasets in the job's order.
pub fn admit(store: &Foundry, spec: &Spec) -> Result<Vec<Dataset>, Refused> {
    store.read(|state| {
        let recipe = state
            .recipes
            .get(&spec.recipe)
            .ok_or((404, format!("no recipe {}", spec.recipe)))?;
        if recipe.vetted_ms.is_none() || recipe.commit.is_empty() {
            return Err((
                409,
                format!(
                    "recipe {} is not vetted: the owner vets a commit first",
                    spec.recipe
                ),
            ));
        }
        let mut found = Vec::new();
        for id in &spec.datasets {
            let dataset = state
                .datasets
                .get(id)
                .ok_or((404, format!("no dataset {id}")))?;
            if let Some(flag) = dataset.taint.iter().find(|flag| datasets::blocking(flag)) {
                return Err((
                    409,
                    format!("dataset {id} is tainted ({flag}) and cannot train a job"),
                ));
            }
            if dataset.rows == 0 {
                return Err((409, format!("dataset {id} has no training rows")));
            }
            found.push(dataset.clone());
        }
        Ok(found)
    })
}

/// The laws' check of `training.submit`, through the same hook the other routes use.
pub fn law_check(app: &crate::state::App, spec: &Spec) -> Result<(), Refused> {
    let (cid, role) = match spec.agent {
        Some(cid) => (cid, app.agents.get(cid).map(|a| a.role).unwrap_or_default()),
        None => (0, "owner".to_string()),
    };
    // S-006c/d are written on `resource is Town` and read `context.gpu_hours`: the same shape as laws.rs.
    let resource = jhrl::authz::Entity {
        ty: "Town".into(),
        id: "main".into(),
    };
    let attrs = std::collections::BTreeMap::from([(
        "recipe".to_string(),
        jhrl::value::Value::str(&spec.recipe),
    )]);
    let extra = std::collections::BTreeMap::from([(
        "gpu_hours".to_string(),
        jhrl::value::Value::int(spec.budget.ceil() as i128),
    )]);
    match app.laws.refuse_op_with(
        cid,
        &role,
        "training.submit",
        resource,
        attrs,
        &json!({"recipe": spec.recipe}),
        extra,
    ) {
        Some(why) => Err((403, why)),
        None => Ok(()),
    }
}

/// The budget check: the cap for every submitter.
pub fn budget_check(spec: &Spec) -> Result<(), Refused> {
    if spec.budget > MAX_BUDGET_GPU_HOURS {
        return Err((
            403,
            format!(
                "budget_gpu_hours {} is over the {MAX_BUDGET_GPU_HOURS} GPU-hour cap",
                spec.budget
            ),
        ));
    }
    Ok(())
}

/// The planner over the fleet's machines. A planner error is a 422 with its own JSON.
pub fn place(machines: &[Machine], reach: &Reach, spec: &Spec) -> Result<Plan, Refused> {
    plan::plan(machines, reach, &spec.request).map_err(|e| (422, e.to_string()))
}

/// The plan document the trainer reads: the planner's plan with the container paths, the dataset
/// refs mapped to `/job/data`, the frozen set of the first dataset and the recipe's train overrides.
pub fn plan_document(plan: &Plan, spec: &Spec, datasets: &[Dataset]) -> Value {
    let mut map = serde_json::Map::new();
    for ds in datasets {
        map.insert(ds.id.clone(), json!(format!("/job/data/{}.jsonl", ds.id)));
    }
    let frozen = datasets
        .first()
        .map(|ds| format!("/job/data/{}.frozen.jsonl", ds.id));
    let steps = spec.train.get("steps").and_then(Value::as_u64);
    let mut doc = json!({
        "mode": plan.mode.to_string(),
        "islands": plan.islands,
        "inner_steps": plan.inner_steps,
        "staleness": plan.staleness,
        "devices_total": plan.devices_total,
        "datasets": map,
        "frozen": frozen,
        "output_dir": "/job/out",
        "train": spec.train,
    });
    if plan.mode == Mode::Loose
        && let Some(steps) = steps
    {
        doc["rounds"] = json!((steps / u64::from(plan.inner_steps)).max(1));
    }
    doc
}

/// The container image for a backend: `apz-trainer:<backend>`. The native mps runner has no
/// mounts, so it cannot take a job yet.
pub fn image_for(backend: &str) -> Result<String, String> {
    match backend {
        "cuda" | "rocm" | "xpu" | "cpu" => Ok(format!("{IMAGE}:{backend}")),
        "mps" => Err(
            "mps islands run the native runner, which takes no mounts: not supported yet".into(),
        ),
        other => Err(format!("unknown backend {other:?}")),
    }
}

/// Writes the job directory: `job.json`, `plan.json`, `data/` (the datasets' files) and `out/`.
pub fn write_job_dir(
    store: &Foundry,
    id: &str,
    spec_doc: &Value,
    plan_doc: &Value,
    datasets: &[Dataset],
) -> Result<PathBuf, Refused> {
    let dir = store.jobs_dir().join(id);
    let fail = |e: std::io::Error| (500, format!("job directory {}: {e}", dir.display()));
    std::fs::create_dir_all(dir.join("data")).map_err(fail)?;
    std::fs::create_dir_all(dir.join("out")).map_err(fail)?;
    std::fs::write(dir.join("job.json"), spec_doc.to_string()).map_err(fail)?;
    std::fs::write(dir.join("plan.json"), plan_doc.to_string()).map_err(fail)?;
    for ds in datasets {
        let from = store.dir.join(&ds.file);
        let to = dir.join("data").join(format!("{}.jsonl", ds.id));
        link_or_copy(&from, &to).map_err(fail)?;
        let frozen_to = dir.join("data").join(format!("{}.frozen.jsonl", ds.id));
        link_or_copy(&store.dir.join(&ds.frozen_file), &frozen_to).map_err(fail)?;
    }
    Ok(dir)
}

fn link_or_copy(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.exists() {
        std::fs::remove_file(to)?;
    }
    std::fs::hard_link(from, to).or_else(|_| std::fs::copy(from, to).map(|_| ()))
}

/// Starts every island's run on its machine's noded. Multi-machine islands are refused: the
/// torchrun launch across machines is not wired yet. A failed start stops and deletes the runs
/// already begun, so nothing is left running for a job that failed.
pub fn dispatch(
    calls: &dyn NodeCalls,
    job: &str,
    plan: &Plan,
    dir: &Path,
    recipe_host: &Path,
    hub_base: &str,
    now: u64,
) -> Result<Vec<Run>, Refused> {
    let mut started: Vec<Run> = Vec::new();
    for island in &plan.islands {
        if island.machines.len() != 1 {
            rollback(calls, &started);
            return Err((
                422,
                format!(
                    "island {} spans {} machines: multi-machine islands are not wired yet",
                    island.id,
                    island.machines.len()
                ),
            ));
        }
        let placement = &island.machines[0];
        let image = match image_for(&island.backend) {
            Ok(image) => image,
            Err(reason) => {
                rollback(calls, &started);
                return Err((422, reason));
            }
        };
        let loose = plan.mode == Mode::Loose;
        let node_job = format!("{job}-{}", island.id);
        let devices: Vec<u32> = if island.backend == "cpu" {
            Vec::new()
        } else {
            placement.devices.clone()
        };
        let mut args = vec![
            "--job",
            "/job/job.json",
            "--plan",
            "/job/plan.json",
            "--island",
            island.id.as_str(),
            "--out",
            "/job/out",
            "--recipes",
            "/recipes",
            "--data-dir",
            "/job/data",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<String>>();
        if loose {
            args.push("--hub".into());
            args.push(format!("{hub_base}/v1/foundry/jobs/{job}/hub"));
        }
        let body = json!({
            "job_id": node_job,
            "image": image,
            "devices": devices,
            "env": {"PYTHONUNBUFFERED": "1"},
            "args": args,
            "mounts": [
                {"host": dir.display().to_string(), "container": "/job", "ro": false},
                {"host": recipe_host.display().to_string(), "container": "/recipes", "ro": true},
            ],
            "network": if loose { "host" } else { "none" },
        });
        match calls.call(&placement.id, "POST", "/v1/train", Some(&body)) {
            Ok((201, _)) => started.push(Run {
                node_job,
                node: placement.id.clone(),
                island: island.id.clone(),
                devices,
                started_ms: now,
                ended_ms: None,
                state: "running".into(),
                exit_code: None,
                next: 0,
                gpu_hours: 0.0,
            }),
            Ok((status, reply)) => {
                rollback(calls, &started);
                return Err((
                    502,
                    format!(
                        "{} refused the run ({status}): {}",
                        placement.id,
                        reply["error"].as_str().unwrap_or("no reason given")
                    ),
                ));
            }
            Err(err) => {
                rollback(calls, &started);
                return Err((502, format!("{} did not answer: {err}", placement.id)));
            }
        }
    }
    Ok(started)
}

/// Stops and deletes the runs already started (best effort).
fn rollback(calls: &dyn NodeCalls, runs: &[Run]) {
    for run in runs {
        let _ = calls.call(
            &run.node,
            "POST",
            &format!("/v1/train/{}/stop", run.node_job),
            None,
        );
        let _ = calls.call(
            &run.node,
            "DELETE",
            &format!("/v1/train/{}", run.node_job),
            None,
        );
    }
}

/// The events of one job, appended to its `events.jsonl` with a sequence number. Returns the
/// lines written, for the bus.
pub fn poll(
    calls: &dyn NodeCalls,
    store: &Foundry,
    models: &crate::foundry::registry::Models,
    job_id: &str,
    now: u64,
) -> Vec<Value> {
    let Some(mut job) = store.read(|s| s.jobs.get(job_id).cloned()) else {
        return Vec::new();
    };
    if job.status != "running" {
        return Vec::new();
    }
    let dir = store.jobs_dir().join(job_id);
    let mut out = Vec::new();
    let mut runs = std::mem::take(&mut job.runs);
    for run in runs.iter_mut().filter(|r| r.state == "running") {
        let path = format!("/v1/train/{}/events?after={}", run.node_job, run.next);
        let Ok((200, reply)) = calls.call(&run.node, "GET", &path, None) else {
            continue;
        };
        for event in reply["events"].as_array().cloned().unwrap_or_default() {
            job.seq += 1;
            let line =
                json!({"seq": job.seq, "island": run.island, "node": run.node, "event": event});
            append_line(&dir.join("events.jsonl"), &line);
            absorb(&mut job, &run.island, &event, now);
            out.push(line);
        }
        run.next = reply["next"].as_u64().unwrap_or(run.next);
        if reply["state"] == "exited" {
            run.state = "exited".into();
            run.exit_code = reply["exit_code"].as_i64();
            run.ended_ms = Some(now);
            run.gpu_hours = meter(run, now);
            let _ = calls.call(
                &run.node,
                "DELETE",
                &format!("/v1/train/{}", run.node_job),
                None,
            );
        }
    }
    job.runs = runs;
    job.gpu_hours = job.runs.iter().map(|r| r.gpu_hours).sum::<f64>() + running_hours(&job, now);
    job.updated_ms = now;
    finish_if_due(calls, &mut job, &dir, now);
    job.outputs = list_outputs(&dir.join("out"));
    let done = job.status == "done";
    store.write().jobs.insert(job_id.to_string(), job);
    if done {
        register_finished(models, store, job_id, &dir.join("out"));
    }
    out
}

/// Registers a finished job's model as `trained`, once: a job whose model is already in the
/// registry (matched by its source job) is skipped. A failure is logged and kept on the job's
/// error field; the job stays `done`.
fn register_finished(
    models: &crate::foundry::registry::Models,
    store: &Foundry,
    job_id: &str,
    out: &Path,
) {
    if models
        .list()
        .iter()
        .any(|m| m.source_job.as_deref() == Some(job_id))
    {
        return;
    }
    let (dir, candidates) = pick_model(out);
    let registered = match dir {
        Some(dir) => models.register_from_job(&dir, Some(job_id)).map(|_| ()),
        None => Err("no model_card.json in the job's output".to_string()),
    };
    let mut state = store.write();
    let Some(job) = state.jobs.get_mut(job_id) else {
        return;
    };
    job.candidates = candidates;
    if let Err(e) = registered {
        eprintln!("foundry: job {job_id}: the model was not registered: {e}");
        job.error = Some(format!("model not registered: {e}"));
    }
}

/// The directory a finished job's model is registered from, and the islands whose models are
/// kept as candidates instead. A top-level `out/model_card.json` (a merged or outer model) wins
/// outright. Otherwise each island directory under `out/` with a card is ranked by its validation
/// eval (`metrics.val`): the higher `accuracy` wins; a card without accuracy ranks by its `nll`
/// (lower is better, so it scores as `-nll`), then by its `loss` (also `-loss`); a card with none
/// of these ranks last. Ties go to the
/// lower island id. A single island is the winner with no candidates.
fn pick_model(out: &Path) -> (Option<PathBuf>, Vec<String>) {
    if out.join("model_card.json").is_file() {
        return (Some(out.to_path_buf()), Vec::new());
    }
    let Ok(entries) = std::fs::read_dir(out) else {
        return (None, Vec::new());
    };
    let mut ranked: Vec<(Option<f64>, String, PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().join("model_card.json").is_file())
        .filter_map(|e| {
            let id = e.file_name().into_string().ok()?;
            Some((card_score(&e.path().join("model_card.json")), id, e.path()))
        })
        .collect();
    let rank = |score: Option<f64>| score.unwrap_or(f64::NEG_INFINITY);
    ranked.sort_by(|a, b| rank(b.0).total_cmp(&rank(a.0)).then_with(|| a.1.cmp(&b.1)));
    let candidates = ranked.iter().skip(1).map(|r| r.1.clone()).collect();
    (ranked.first().map(|r| r.2.clone()), candidates)
}

/// The eval score of a trainer card, higher is better (see [`pick_model`]).
fn card_score(card_path: &Path) -> Option<f64> {
    let card: Value = serde_json::from_str(&std::fs::read_to_string(card_path).ok()?).ok()?;
    let val = &card["metrics"]["val"];
    val["accuracy"]
        .as_f64()
        .or_else(|| val["nll"].as_f64().map(|nll| -nll))
        .or_else(|| val["loss"].as_f64().map(|loss| -loss))
}

/// Device time from dispatch to `now`, in GPU-hours, for one run.
pub fn meter(run: &Run, now: u64) -> f64 {
    let end = run.ended_ms.unwrap_or(now);
    run.devices.len().max(1) as f64 * end.saturating_sub(run.started_ms) as f64 / 3_600_000.0
}

fn running_hours(job: &Job, now: u64) -> f64 {
    job.runs
        .iter()
        .filter(|r| r.state == "running")
        .map(|r| meter(r, now))
        .sum()
}

/// Folds one trainer event into the job: the series, the metrics, the error.
pub fn absorb(job: &mut Job, island: &str, event: &Value, now: u64) {
    let ev = event["ev"].as_str().unwrap_or_default();
    let island = island.to_string();
    let point = match ev {
        "step" => Some(Point::Step {
            island,
            step: event["step"].as_u64(),
            loss: event["loss"].as_f64(),
            lr: event["lr"].as_f64(),
            samples: event["samples"].as_u64(),
            t: now,
        }),
        "eval" => Some(Point::Eval {
            island,
            step: event["step"].as_u64(),
            split: event["split"].as_str().map(str::to_string),
            metrics: event["metrics"].clone(),
            t: now,
        }),
        "round" => Some(Point::Round {
            island,
            round: event["round"].as_u64(),
            accepted: event["accepted"].as_bool(),
            hub_round: event["hub_round"].as_u64(),
            step: event["step"].as_u64(),
            t: now,
        }),
        "trial" => Some(Point::Trial {
            island,
            trial: event["trial"].as_u64(),
            objective: event["objective"].as_f64(),
            t: now,
        }),
        _ => None,
    };
    if let Some(point) = point {
        job.series.push(point);
        let excess = job.series.len().saturating_sub(SERIES_CAP);
        job.series.drain(..excess);
    }
    match ev {
        "done" => job.metrics = event.get("metrics").cloned().unwrap_or(Value::Null),
        "error" => {
            job.error = Some(
                event["message"]
                    .as_str()
                    .unwrap_or("the trainer failed")
                    .to_string(),
            )
        }
        _ => {}
    }
}

/// Ends the job when its budget is spent (stopping its runs) or when every run has exited.
fn finish_if_due(calls: &dyn NodeCalls, job: &mut Job, dir: &Path, now: u64) {
    if job.gpu_hours > job.budget_gpu_hours {
        for run in job.runs.iter_mut().filter(|r| r.state == "running") {
            let _ = calls.call(
                &run.node,
                "POST",
                &format!("/v1/train/{}/stop", run.node_job),
                None,
            );
            let _ = calls.call(
                &run.node,
                "DELETE",
                &format!("/v1/train/{}", run.node_job),
                None,
            );
            run.state = "stopped".into();
            run.ended_ms = Some(now);
            run.gpu_hours = meter(run, now);
        }
        job.status = "failed".into();
        job.error = Some(format!(
            "budget exhausted: {:.3} of {} GPU-hours",
            job.gpu_hours, job.budget_gpu_hours
        ));
        job.gpu_hours = job.runs.iter().map(|r| r.gpu_hours).sum();
        return;
    }
    if job.runs.iter().any(|r| r.state == "running") || job.runs.is_empty() {
        return;
    }
    let failed = job.runs.iter().any(|r| r.exit_code != Some(0));
    if failed {
        job.status = "failed".into();
        job.error = job
            .error
            .clone()
            .or(Some("a run exited with an error".into()));
    } else {
        job.status = "done".into();
        job.error = None;
    }
    job.outputs_remote = job.runs.iter().any(|r| r.node != crate::fleet::LOCAL);
    let _ = dir;
}

/// Cancels a job: each running run is stopped and deleted.
pub fn cancel(calls: &dyn NodeCalls, store: &Foundry, job_id: &str, now: u64) -> Option<Job> {
    let mut job = store.read(|s| s.jobs.get(job_id).cloned())?;
    if job.status != "running" && job.status != "queued" {
        return Some(job);
    }
    for run in job.runs.iter_mut().filter(|r| r.state == "running") {
        let _ = calls.call(
            &run.node,
            "POST",
            &format!("/v1/train/{}/stop", run.node_job),
            None,
        );
        let _ = calls.call(
            &run.node,
            "DELETE",
            &format!("/v1/train/{}", run.node_job),
            None,
        );
        run.state = "stopped".into();
        run.ended_ms = Some(now);
        run.gpu_hours = meter(run, now);
    }
    job.gpu_hours = job.runs.iter().map(|r| r.gpu_hours).sum();
    job.status = "cancelled".into();
    job.updated_ms = now;
    store.write().jobs.insert(job_id.to_string(), job.clone());
    Some(job)
}

/// The names of the files in a job's output directory, sorted.
pub fn list_outputs(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|e| e.path().is_file())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn append_line(path: &Path, line: &Value) {
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foundry::plan::{Accelerator, Island, Placement};
    use std::cell::RefCell;

    /// A noded that answers from a script and records the calls.
    struct Fake {
        calls: RefCell<Vec<(String, String, String)>>,
        /// The starts from this one (0-based) on are refused.
        fail_start_from: usize,
        starts: RefCell<usize>,
        events: Value,
    }

    impl Fake {
        fn new(fail_start_from: usize, events: Value) -> Self {
            Fake {
                calls: RefCell::new(Vec::new()),
                fail_start_from,
                starts: RefCell::new(0),
                events,
            }
        }
    }

    impl NodeCalls for Fake {
        fn call(
            &self,
            node: &str,
            method: &str,
            path: &str,
            _body: Option<&Value>,
        ) -> Result<(u16, Value), String> {
            self.calls
                .borrow_mut()
                .push((node.into(), method.into(), path.into()));
            Ok(match (method, path) {
                ("POST", "/v1/train") => {
                    let index = *self.starts.borrow();
                    *self.starts.borrow_mut() += 1;
                    if index >= self.fail_start_from {
                        (409, json!({"error": "job exists"}))
                    } else {
                        (201, json!({"container": "c"}))
                    }
                }
                ("GET", _) => (200, self.events.clone()),
                _ => (200, json!({"deleted": true})),
            })
        }
    }

    fn spec(budget: f64) -> Spec {
        let doc = json!({"job_id": "tj-test", "recipe": "rung-predictor@1", "datasets": ["ds-1"], "budget_gpu_hours": budget, "train": {"steps": 1000}});
        parse(&doc, new_job_id).expect("parses")
    }

    fn gpu_plan(mode: Mode, machines: usize) -> Plan {
        let islands = (0..machines)
            .map(|i| Island {
                id: format!("i{}", i + 1),
                backend: "cuda".into(),
                machines: vec![Placement {
                    id: format!("m{i}"),
                    devices: vec![0],
                }],
                leader: format!("m{i}"),
                rdzv: None,
            })
            .collect();
        Plan {
            mode,
            islands,
            inner_steps: 500,
            staleness: 0,
            devices_total: machines as u32,
        }
    }

    #[test]
    fn a_good_document_parses_and_a_bad_one_is_refused() {
        assert!(parse(&json!({"job_id": "a", "recipe": "rung-predictor@1", "datasets": ["x"], "budget_gpu_hours": 2}), new_job_id).is_ok());
        let refused = |doc: Value| parse(&doc, new_job_id).unwrap_err().0;
        assert_eq!(
            refused(
                json!({"job_id": "-bad", "recipe": "rung-predictor@1", "datasets": ["x"], "budget_gpu_hours": 2})
            ),
            400
        );
        assert_eq!(
            refused(json!({"recipe": "Rung@1", "datasets": ["x"], "budget_gpu_hours": 2})),
            400
        );
        assert_eq!(
            refused(json!({"recipe": "rung-predictor@1", "datasets": [], "budget_gpu_hours": 2})),
            400
        );
        assert_eq!(
            refused(
                json!({"recipe": "rung-predictor@1", "datasets": ["x"], "budget_gpu_hours": 0})
            ),
            400
        );
        assert_eq!(
            refused(
                json!({"recipe": "rung-predictor@1", "datasets": ["x"], "budget_gpu_hours": 2, "rogue": 1})
            ),
            400
        );
        assert_eq!(
            refused(
                json!({"recipe": "rung-predictor@1", "datasets": ["x"], "budget_gpu_hours": 2, "mode": "sideways"})
            ),
            400
        );
    }

    #[test]
    fn the_budget_is_capped_at_eight_gpu_hours() {
        assert!(budget_check(&spec(8.0)).is_ok());
        assert_eq!(budget_check(&spec(9.0)).unwrap_err().0, 403);
    }

    #[test]
    fn the_device_block_maps_to_the_planner_request() {
        let doc = json!({"recipe": "rung-predictor@1", "datasets": ["x"], "budget_gpu_hours": 1,
            "devices": {"min_devices": 2, "max_devices": 4, "backends": ["cuda"]}, "mode": "loose"});
        let parsed = parse(&doc, new_job_id).expect("parses");
        assert_eq!(parsed.request.min_devices, 2);
        assert_eq!(parsed.request.max_devices, 4);
        assert_eq!(parsed.request.mode, Some(Mode::Loose));
        assert_eq!(parsed.request.budget_gpu_hours, Some(1.0));
        let plain = spec(1.0);
        assert_eq!(
            (plain.request.min_devices, plain.request.max_devices),
            (1, 1)
        );
    }

    #[test]
    fn a_planner_error_is_a_422() {
        let machines = vec![Machine {
            id: "n0".into(),
            online: true,
            accelerators: vec![Accelerator::default()],
            trainer_ok: true,
            fabric: vec![],
        }];
        let mut bad = spec(1.0);
        bad.request.backends = Some(vec!["cuda".into()]);
        bad.request.min_devices = 3;
        bad.request.max_devices = 3;
        assert_eq!(
            place(&machines, &Reach::default(), &bad).unwrap_err().0,
            422
        );
    }

    #[test]
    fn the_plan_document_points_at_the_container_paths() {
        let ds = Dataset {
            id: "ds-1".into(),
            file: "datasets/ds-1.jsonl".into(),
            frozen_file: "datasets/ds-1.frozen.jsonl".into(),
            ..Dataset::default()
        };
        let doc = plan_document(&gpu_plan(Mode::Loose, 2), &spec(1.0), &[ds]);
        assert_eq!(doc["datasets"]["ds-1"], "/job/data/ds-1.jsonl");
        assert_eq!(doc["frozen"], "/job/data/ds-1.frozen.jsonl");
        assert_eq!(doc["output_dir"], "/job/out");
        assert_eq!(doc["rounds"], 2);
    }

    #[test]
    fn dispatch_posts_each_island_and_a_failed_start_rolls_back() {
        let dir = std::env::temp_dir().join(format!("foundry-dispatch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let fake = Fake::new(usize::MAX, json!({}));
        let plan = gpu_plan(Mode::Loose, 1);
        let runs = dispatch(
            &fake,
            "tj-test",
            &plan,
            &dir.join("jobs/tj-test"),
            &dir,
            "http://127.0.0.1:1",
            5,
        )
        .expect("starts");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].node_job, "tj-test-i1");
        // Two islands, the second start refused: the first run is stopped and deleted.
        let failing = Fake::new(1, json!({}));
        let two = gpu_plan(Mode::Tight, 2);
        let err = dispatch(&failing, "tj-test", &two, &dir, &dir, "", 5).unwrap_err();
        assert_eq!(err.0, 502);
        let calls = failing.calls.borrow();
        assert!(
            calls
                .iter()
                .any(|(_, m, p)| m == "POST" && p == "/v1/train/tj-test-i1/stop")
        );
        assert!(
            calls
                .iter()
                .any(|(_, m, p)| m == "DELETE" && p == "/v1/train/tj-test-i1")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_polled_exit_marks_the_job_done_and_meters_the_device_time() {
        let dir = std::env::temp_dir().join(format!("foundry-poll-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("jobs/tj-poll")).expect("dir");
        let store = crate::foundry::store::open_for_test(dir.clone());
        let events = json!({"state": "exited", "exit_code": 0, "next": 2, "events": [
            {"ev": "step", "step": 10, "loss": 0.5, "lr": 0.002, "samples": 256, "line": 1},
            {"ev": "done", "metrics": {"val": {"loss": 0.4}}, "line": 2}]});
        let fake = Fake::new(usize::MAX, events);
        store
            .write()
            .jobs
            .insert("tj-poll".into(), running("tj-poll"));
        let models = crate::foundry::registry::Models::open(&dir.join("state"));
        let lines = poll(&fake, &store, &models, "tj-poll", 3_600_000);
        assert_eq!(lines.len(), 2);
        let done = store.read(|s| s.jobs["tj-poll"].clone());
        assert_eq!(done.status, "done");
        assert_eq!(done.series.len(), 1);
        assert!(
            (done.gpu_hours - 1.0).abs() < 1e-9,
            "one device for one hour"
        );
        assert!(fake.calls.borrow().iter().any(|(_, m, _)| m == "DELETE"));
        let _ = std::fs::remove_dir_all(dir);
    }

    const LINEAR3: &[u8] = include_bytes!("../../tests/fixtures/linear3.onnx");

    /// A running job with one run on noded `n0`, not yet polled.
    fn running(id: &str) -> Job {
        Job {
            id: id.into(),
            status: "running".into(),
            created_ms: 0,
            updated_ms: 0,
            recipe: "rung-predictor@1".into(),
            datasets: vec![],
            mode: "single".into(),
            budget_gpu_hours: 1.0,
            gpu_hours: 0.0,
            agent: None,
            doc: json!({}),
            plan: json!({}),
            runs: vec![Run {
                node_job: format!("{id}-i1"),
                node: "n0".into(),
                island: "i1".into(),
                devices: vec![0],
                started_ms: 0,
                ended_ms: None,
                state: "running".into(),
                exit_code: None,
                next: 0,
                gpu_hours: 0.0,
            }],
            series: vec![],
            seq: 0,
            outputs: vec![],
            outputs_remote: false,
            metrics: json!(null),
            error: None,
            candidates: Vec::new(),
        }
    }

    /// The events of a run that exited with `exit_code`, ending in the `done` event with a model card.
    fn exit_with(exit_code: i64) -> Value {
        json!({"state": "exited", "exit_code": exit_code, "next": 2, "events": [
            {"ev": "done", "metrics": {"val": {"accuracy": 0.9}}, "line": 2}]})
    }

    /// The serving contract a trained card carries: three numeric inputs, three labels.
    fn serving_contract() -> Value {
        json!({
            "input": {"type": "object", "properties": {"x0": {"type": "number"}, "x1": {"type": "number"}, "x2": {"type": "number"}}, "required": ["x0", "x1", "x2"]},
            "output": {"type": "object", "properties": {"label": {"type": "string", "enum": ["cat", "dog", "bird"]}, "probabilities": {"type": "object"}}, "required": ["label", "probabilities"]},
            "features": [{"name": "x0", "kind": "numeric"}, {"name": "x1", "kind": "numeric"}, {"name": "x2", "kind": "numeric"}],
            "labels": ["cat", "dog", "bird"],
            "allowed_roles": ["breeder"]
        })
    }

    /// A job's output directory as the trainer leaves it: the model, and its card when given.
    fn write_outputs(store: &Foundry, id: &str, with_card: bool) {
        let out = store.jobs_dir().join(id).join("out");
        std::fs::create_dir_all(&out).expect("out");
        std::fs::write(out.join("model.onnx"), LINEAR3).expect("onnx");
        if with_card {
            let card = json!({"recipe": "rung_predictor", "job_id": id, "metrics": {"val": {"accuracy": 0.9}}, "serving": serving_contract()});
            std::fs::write(out.join("model_card.json"), card.to_string()).expect("card");
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("foundry-jobs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("state")).expect("dir");
        dir
    }

    #[test]
    fn a_finished_job_registers_its_model_once_even_when_polled_twice() {
        let dir = scratch("once");
        let store = crate::foundry::store::open_for_test(dir.clone());
        let models = crate::foundry::registry::Models::open(&dir.join("state"));
        write_outputs(&store, "tj-once", true);
        store
            .write()
            .jobs
            .insert("tj-once".into(), running("tj-once"));
        let fake = Fake::new(usize::MAX, exit_with(0));
        poll(&fake, &store, &models, "tj-once", 3_600_000);
        poll(&fake, &store, &models, "tj-once", 3_600_000);
        let registered = models.list();
        assert_eq!(registered.len(), 1, "one model, registered once");
        assert_eq!(registered[0].source_job.as_deref(), Some("tj-once"));
        assert_eq!(registered[0].state.name(), "trained");
        let job = store.read(|s| s.jobs["tj-once"].clone());
        assert_eq!(job.status, "done");
        assert!(job.error.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// One island's output as the trainer leaves it: `out/<island>/` with the model and a card
    /// whose validation accuracy is `accuracy`.
    fn write_island(store: &Foundry, id: &str, island: &str, accuracy: f64) {
        let out = store.jobs_dir().join(id).join("out").join(island);
        std::fs::create_dir_all(&out).expect("island out");
        std::fs::write(out.join("model.onnx"), LINEAR3).expect("onnx");
        let card = json!({"recipe": "rung_predictor", "job_id": id, "metrics": {"val": {"accuracy": accuracy}}, "serving": serving_contract()});
        std::fs::write(out.join("model_card.json"), card.to_string()).expect("card");
    }

    #[test]
    fn a_card_with_only_nll_ranks_by_the_lower_nll() {
        let dir = scratch("nll");
        let out = dir.join("out");
        for (island, nll) in [("i1", 0.5), ("i2", 0.3)] {
            let island_dir = out.join(island);
            std::fs::create_dir_all(&island_dir).expect("island out");
            let card = json!({"metrics": {"val": {"nll": nll}}});
            std::fs::write(island_dir.join("model_card.json"), card.to_string()).expect("card");
        }
        let (best, candidates) = pick_model(&out);
        let want = out.join("i2");
        assert_eq!(best, Some(want), "the lower nll wins");
        assert_eq!(candidates, vec!["i1".to_string()]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_finished_job_registers_the_best_island_from_its_island_directories() {
        // (case, islands as (name, accuracy), the accuracy of the island registered, the islands kept as candidates).
        // One island is the nested layout with nothing else to keep; two islands keep the other as a candidate.
        let cases: [(&str, &[(&str, f64)], f64, Vec<&str>); 2] = [
            ("one island", &[("i1", 0.9)], 0.9, vec![]),
            ("two islands", &[("i1", 0.7), ("i2", 0.9)], 0.9, vec!["i1"]),
        ];
        for (case, islands, best, candidates) in cases {
            let dir = scratch(&case.replace(' ', "-"));
            let store = crate::foundry::store::open_for_test(dir.clone());
            let models = crate::foundry::registry::Models::open(&dir.join("state"));
            for (island, accuracy) in islands {
                write_island(&store, "tj-islands", island, *accuracy);
            }
            store
                .write()
                .jobs
                .insert("tj-islands".into(), running("tj-islands"));
            let fake = Fake::new(usize::MAX, exit_with(0));
            poll(&fake, &store, &models, "tj-islands", 3_600_000);
            let registered = models.list();
            assert_eq!(registered.len(), 1, "{case}: one model, the best island");
            assert_eq!(registered[0].source_job.as_deref(), Some("tj-islands"));
            assert_eq!(
                registered[0].card.metrics["val"]["accuracy"],
                json!(best),
                "{case}: the best island is registered"
            );
            let job = store.read(|s| s.jobs["tj-islands"].clone());
            assert_eq!(job.status, "done", "{case}");
            assert!(job.error.is_none(), "{case}");
            assert_eq!(job.candidates, candidates, "{case}: the other islands");
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn a_failed_job_does_not_register() {
        let dir = scratch("failed");
        let store = crate::foundry::store::open_for_test(dir.clone());
        let models = crate::foundry::registry::Models::open(&dir.join("state"));
        write_outputs(&store, "tj-failed", true);
        store
            .write()
            .jobs
            .insert("tj-failed".into(), running("tj-failed"));
        let fake = Fake::new(usize::MAX, exit_with(1));
        poll(&fake, &store, &models, "tj-failed", 3_600_000);
        assert_eq!(store.read(|s| s.jobs["tj-failed"].status.clone()), "failed");
        assert!(models.list().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_registration_error_is_recorded_and_the_job_stays_done() {
        let dir = scratch("register-error");
        let store = crate::foundry::store::open_for_test(dir.clone());
        let models = crate::foundry::registry::Models::open(&dir.join("state"));
        write_outputs(&store, "tj-noncard", false);
        store
            .write()
            .jobs
            .insert("tj-noncard".into(), running("tj-noncard"));
        let fake = Fake::new(usize::MAX, exit_with(0));
        poll(&fake, &store, &models, "tj-noncard", 3_600_000);
        let job = store.read(|s| s.jobs["tj-noncard"].clone());
        assert_eq!(job.status, "done");
        assert!(
            job.error
                .unwrap_or_default()
                .starts_with("model not registered")
        );
        assert!(models.list().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_untrusted_taint_blocks_a_job_and_a_clean_dataset_passes() {
        let dir = std::env::temp_dir().join(format!("foundry-admit-{}", std::process::id()));
        let store = crate::foundry::store::open_for_test(dir.clone());
        let base = Dataset {
            id: "ds-1".into(),
            rows: 300,
            ..Dataset::default()
        };
        store.write().datasets.insert("ds-1".into(), base.clone());
        assert!(admit(&store, &spec(1.0)).is_ok());
        store.write().datasets.insert(
            "ds-1".into(),
            Dataset {
                taint: vec!["unverified_upload".into()],
                ..base.clone()
            },
        );
        assert_eq!(admit(&store, &spec(1.0)).unwrap_err().0, 409);
        store
            .write()
            .datasets
            .insert("ds-1".into(), Dataset { rows: 0, ..base });
        assert_eq!(admit(&store, &spec(1.0)).unwrap_err().0, 409);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_unvetted_recipe_is_refused() {
        let dir = std::env::temp_dir().join(format!("foundry-vet-{}", std::process::id()));
        let store = crate::foundry::store::open_for_test(dir.clone());
        store
            .write()
            .recipes
            .get_mut(crate::foundry::store::SEED_RECIPE)
            .expect("seed")
            .vetted_ms = None;
        store.write().datasets.insert(
            "ds-1".into(),
            Dataset {
                id: "ds-1".into(),
                rows: 5,
                ..Dataset::default()
            },
        );
        assert_eq!(admit(&store, &spec(1.0)).unwrap_err().0, 409);
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod law_refusal_tests {
    use crate::laws::Laws;
    use std::collections::BTreeMap;

    /// The `training.submit` check as `law_check` makes it: the Town resource and `gpu_hours` in context.
    fn refuse(laws: &Laws, gpu_hours: i128) -> Option<String> {
        let town = jhrl::authz::Entity {
            ty: "Town".into(),
            id: "main".into(),
        };
        let extra = BTreeMap::from([("gpu_hours".to_string(), jhrl::value::Value::int(gpu_hours))]);
        laws.refuse_op_with(
            1,
            "breeder",
            "training.submit",
            town,
            BTreeMap::new(),
            &serde_json::json!({}),
            extra,
        )
    }

    #[test]
    fn a_breeder_job_over_eight_gpu_hours_is_refused_by_the_statute_and_one_within_passes() {
        let laws = Laws::builtin();
        let over = refuse(&laws, 9).expect("a 9 GPU-hour job is refused by law");
        assert!(over.contains("S-006"), "{over}");
        assert!(
            refuse(&laws, 2).is_none(),
            "a 2 GPU-hour job passes the statute"
        );
    }
}
