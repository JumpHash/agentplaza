//! The Lab's calls to apid (`/v1/foundry/...`): recipes, datasets, the device fleet, jobs and the
//! model registry, as typed structs over the same JSON the web console reads (`apps/web/src/api.ts`,
//! the Foundry section). Every function takes the console's [`Client`]; the names match the web's.
//! A non-2xx answer is an `Err` with the hub's `error` text. A refused dispatch (422) keeps the job.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::client::Client;

// ---- Recipes ---------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    pub repo: String,
    pub commit: String,
    pub schema: Value,
    pub vetted_by: Option<String>,
    pub vetted_ms: Option<u64>,
    pub local_dev: bool,
}

pub fn list_recipes(client: &Client) -> Result<Vec<Recipe>, String> {
    client.get("/v1/foundry/recipes")
}

/// The owner vets a commit: 400 unless `commit` is 7 to 64 hex characters (or `dev-...`), 403 on a local-dev pin mismatch.
pub fn vet_recipe(client: &Client, id: &str, commit: &str) -> Result<Recipe, String> {
    client.post(&format!("/v1/foundry/recipes/{}/vet", enc(id)), &json!({ "commit": commit }))
}

// ---- Datasets --------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Dataset {
    pub id: String,
    pub name: String,
    /// `cascade`, `tasks`, `usage`, `upload`, or `all`.
    pub kind: String,
    pub created_ms: u64,
    pub from_ms: u64,
    pub to_ms: u64,
    pub rows: u64,
    pub frozen_rows: u64,
    pub label_balance: BTreeMap<String, u64>,
    pub taint: Vec<String>,
    pub provenance: Value,
    pub file: String,
    pub frozen_file: String,
    pub sha256: String,
}

/// An export from the plaza's records. `since_ms` is inclusive and `until_ms` exclusive, both on a row's `ts`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DatasetSpec {
    pub kind: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until_ms: Option<u64>,
}

pub fn list_datasets(client: &Client) -> Result<Vec<Dataset>, String> {
    client.get("/v1/foundry/datasets")
}

pub fn create_dataset(client: &Client, spec: &DatasetSpec) -> Result<Dataset, String> {
    let body = serde_json::to_value(spec).map_err(|e| e.to_string())?;
    client.post("/v1/foundry/datasets", &body)
}

/// Imports a JSONL body as a dataset. Each row needs `ts` (unix ms), `label` and the recipe's feature fields.
pub fn upload_dataset(client: &Client, name: &str, jsonl: &str) -> Result<Dataset, String> {
    let path = format!("/v1/foundry/datasets/upload?name={}", enc(name));
    let reply = apz_http::client::request(&client.target, "POST", &path, Some(jsonl.as_bytes()), client.timeout).map_err(|e| e.to_string())?;
    let value = if reply.body.is_empty() { Value::Null } else { reply.json().map_err(|e| e.to_string())? };
    answered(reply.status, value)
}

// ---- Devices and planning --------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Device {
    pub index: u32,
    pub vendor: String,
    pub name: String,
    pub backend: String,
    pub memory_mib: u64,
    pub mem_used_mib: u64,
    pub busy: bool,
    pub util_pct: u32,
    /// The job whose running run holds this device, when one does.
    pub allocated_to: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DeviceMachine {
    pub id: String,
    pub online: bool,
    pub trainer_ok: bool,
    pub devices: Vec<Device>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DevicesView {
    pub machines: Vec<DeviceMachine>,
}

pub fn list_devices(client: &Client) -> Result<DevicesView, String> {
    client.get("/v1/foundry/devices")
}

/// What the New job form sends. `recipe` is `name@version`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct JobDocument {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    pub recipe: String,
    pub datasets: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search: Option<Value>,
    pub budget_gpu_hours: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub export: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub devices: Option<DeviceRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub train: Option<Value>,
}

/// The job document's `devices` block: what placement may use.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DeviceRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_devices: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_devices: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_memory_mib: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backends: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub machines: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_cpu: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct IslandMachine {
    pub id: String,
    pub devices: Vec<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Island {
    pub id: String,
    pub backend: String,
    pub leader: String,
    /// The rendezvous address; apid sends null for an island with none (a single-machine island).
    #[serde(deserialize_with = "null_as_empty")]
    pub rdzv: String,
    pub machines: Vec<IslandMachine>,
}

/// A string field that may arrive as null: null reads as the empty string.
fn null_as_empty<'de, D: serde::Deserializer<'de>>(de: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(de)?.unwrap_or_default())
}

/// A job's plan: where it runs and how it trains.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct JobPlan {
    pub mode: String,
    pub islands: Vec<Island>,
    pub inner_steps: u64,
    pub staleness: u64,
    pub devices_total: u64,
    pub datasets: BTreeMap<String, String>,
    pub frozen: Option<String>,
    pub output_dir: String,
    pub train: Value,
    pub rounds: Option<u64>,
}

/// The dry run: the plan a job document would get. Nothing is written or dispatched.
pub fn plan_job(client: &Client, doc: &JobDocument) -> Result<JobPlan, String> {
    let body = serde_json::to_value(doc).map_err(|e| e.to_string())?;
    client.post("/v1/foundry/plan", &body)
}

// ---- Jobs ------------------------------------------------------------------

/// One job in the list, newest first. `runs` is the count of its runs.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct JobSummary {
    pub id: String,
    /// `queued`, `running`, `done`, `failed`, or `cancelled`.
    pub status: String,
    pub mode: String,
    pub recipe: String,
    pub datasets: Vec<String>,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub gpu_hours: f64,
    pub budget_gpu_hours: f64,
    pub error: Option<String>,
    pub runs: u64,
}

/// One machine's share of a plan island.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct JobRun {
    pub node_job: String,
    pub node: String,
    pub island: String,
    pub devices: Vec<u32>,
    pub started_ms: u64,
    pub ended_ms: Option<u64>,
    /// `running`, `exited`, or `stopped`.
    pub state: String,
    pub exit_code: Option<i64>,
    pub next: u64,
    pub gpu_hours: f64,
}

/// One point of a job's series. `kind` is `step`, `eval`, `round`, or `trial`; `t` is the time recorded, ms.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SeriesPoint {
    pub kind: String,
    pub island: String,
    pub t: u64,
    pub step: Option<u64>,
    pub loss: Option<f64>,
    pub lr: Option<f64>,
    pub samples: Option<u64>,
    pub split: Option<String>,
    pub metrics: Option<Value>,
    pub round: Option<u64>,
    pub accepted: Option<bool>,
    pub hub_round: Option<u64>,
    pub trial: Option<u64>,
    pub objective: Option<f64>,
}

/// A job in full. Its `runs` are the run records (the summary's `runs` is their count).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct JobView {
    pub id: String,
    pub status: String,
    pub mode: String,
    pub recipe: String,
    pub datasets: Vec<String>,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub gpu_hours: f64,
    pub budget_gpu_hours: f64,
    pub error: Option<String>,
    pub plan: Option<JobPlan>,
    pub runs: Vec<JobRun>,
    pub series: Vec<SeriesPoint>,
    pub metrics: Option<Value>,
    pub outputs: Vec<String>,
    pub outputs_remote: bool,
    pub seq: u64,
}

/// One stored event: `seq` orders them, `event` is the trainer's own line (its name is `ev`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct JobEvent {
    pub seq: u64,
    pub island: String,
    pub node: String,
    pub event: Value,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct JobEventsPage {
    pub job: String,
    pub events: Vec<JobEvent>,
    pub next: u64,
}

/// A submission apid refused, or one it recorded whose dispatch failed.
#[derive(Debug, Clone)]
pub enum SubmitError {
    /// Refused before a job was recorded (budget, laws, recipe, datasets, or a bad document).
    Refused(String),
    /// Recorded as `failed`: the job comes back with the dispatch's message.
    Dispatch { message: String, job: Box<JobView> },
}

/// Submits a job document and dispatches it (the owner's real machines: only on an explicit request).
pub fn submit_job(client: &Client, doc: &JobDocument) -> Result<JobView, SubmitError> {
    let body = serde_json::to_value(doc).map_err(|e| SubmitError::Refused(e.to_string()))?;
    let (status, value) = apz_http::client::request_json(&client.target, "POST", "/v1/foundry/jobs", Some(&body), client.timeout)
        .map_err(|e| SubmitError::Refused(e.to_string()))?;
    submit_answer(status, value)
}

/// The submit answer. A 2xx is the recorded job. A 422 that carries a `job` is a recorded job whose dispatch
/// failed. Every other refusal, the planner's 422 included (it records no job), keeps the hub's message.
fn submit_answer(status: u16, value: Value) -> Result<JobView, SubmitError> {
    match status {
        200..=299 => serde_json::from_value(value).map_err(|e| SubmitError::Refused(e.to_string())),
        422 => {
            let message = failure(status, &value);
            match value.get("job").filter(|job| !job.is_null()).map(|job| serde_json::from_value::<JobView>(job.clone())) {
                Some(Ok(job)) => Err(SubmitError::Dispatch { message, job: Box::new(job) }),
                _ => Err(SubmitError::Refused(message)),
            }
        }
        _ => Err(SubmitError::Refused(failure(status, &value))),
    }
}

/// Every job, newest first, without the series.
pub fn list_jobs(client: &Client) -> Result<Vec<JobSummary>, String> {
    client.get("/v1/foundry/jobs")
}

pub fn get_job(client: &Client, id: &str) -> Result<JobView, String> {
    client.get(&format!("/v1/foundry/jobs/{}", enc(id)))
}

/// The job's stored events after sequence `after`; keep `next` for the following call.
pub fn job_events(client: &Client, id: &str, after: u64) -> Result<JobEventsPage, String> {
    client.get(&format!("/v1/foundry/jobs/{}/events?after={after}", enc(id)))
}

pub fn cancel_job(client: &Client, id: &str) -> Result<JobView, String> {
    client.post(&format!("/v1/foundry/jobs/{}/cancel", enc(id)), &json!({}))
}

// ---- Models ----------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ModelCard {
    pub id: String,
    pub name: String,
    pub version: String,
    pub input: Value,
    pub output: Value,
    pub features: Vec<Value>,
    pub labels: Option<Vec<String>>,
    pub allowed_roles: Vec<String>,
    pub onnx_sha256: String,
    pub created_ms: u64,
    pub metrics: Value,
    /// The serving contract, when the card has one: for a text model `{kind: "text", task, max_len, labels, graph}`.
    pub serving: Value,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ShadowStats {
    pub runs: u64,
    pub agreed: u64,
    pub cost_saved_usd: f64,
    pub updated_ms: u64,
}

/// A registered model. `state` is `trained`, `shadow`, `promoted`, or `retired`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ModelView {
    pub id: String,
    pub state: String,
    pub card: ModelCard,
    pub source_job: Option<String>,
    pub agent_cid: Option<u64>,
    pub provenance: Value,
    pub calibration: Value,
    /// The record's source: a Hugging Face import (`hf`, `as`, `servable_reason`), or a job's output.
    pub source: Value,
    pub size_bytes: u64,
    pub parameters: u64,
    pub shadow: ShadowStats,
    /// Agreed runs over all shadow runs, 0 to 1; `None` before the first run.
    pub agreement_rate: Option<f64>,
    pub created_ms: u64,
    pub state_changed_ms: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct ModelList {
    models: Vec<ModelView>,
}

pub fn list_models(client: &Client) -> Result<Vec<ModelView>, String> {
    client.get::<ModelList>("/v1/foundry/models").map(|list| list.models)
}

pub fn get_model(client: &Client, id: &str) -> Result<ModelView, String> {
    client.get(&format!("/v1/foundry/models/{}", enc(id)))
}

/// Registers the model in a directory holding `model.onnx` and `model_card.json`; `path` is absolute.
pub fn import_model(client: &Client, path: &str) -> Result<ModelView, String> {
    client.post("/v1/foundry/models/import", &json!({ "path": path }))
}

pub fn shadow_model(client: &Client, id: &str) -> Result<ModelView, String> {
    client.post(&format!("/v1/foundry/models/{}/shadow", enc(id)), &json!({}))
}

pub fn promote_model(client: &Client, id: &str) -> Result<ModelView, String> {
    client.post(&format!("/v1/foundry/models/{}/promote", enc(id)), &json!({}))
}

pub fn retire_model(client: &Client, id: &str) -> Result<ModelView, String> {
    client.post(&format!("/v1/foundry/models/{}/retire", enc(id)), &json!({}))
}

/// The rederive's answer: the model, re-registered from its download, and its state.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Rederived {
    model: ModelView,
}

/// Re-registers an imported model from its download (`POST .../rederive`); only an imported model has one.
pub fn rederive_model(client: &Client, id: &str) -> Result<ModelView, String> {
    client.post::<Rederived>(&format!("/v1/foundry/models/{}/rederive", enc(id)), &json!({})).map(|answer| answer.model)
}

// ---- Shared ----------------------------------------------------------------

/// The answer of a call that must succeed: its JSON, or the hub's `error` text.
fn answered<T: DeserializeOwned>(status: u16, value: Value) -> Result<T, String> {
    if (200..300).contains(&status) {
        serde_json::from_value(value).map_err(|e| e.to_string())
    } else {
        Err(failure(status, &value))
    }
}

/// The hub's `error` text, or the status when it gave none.
fn failure(status: u16, value: &Value) -> String {
    value.get("error").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("HTTP {status}"))
}

/// Percent-encodes a path segment or query value.
fn enc(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            // `@` stays raw: a recipe id is `name@version`, and apid reads the raw form (RFC 3986 allows it).
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'@' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_planner_refusal_keeps_its_message() {
        // The planner's 422 has only `{error}`: no job was recorded, so the message is the answer.
        let refused = submit_answer(422, json!({ "error": "no machine fits 8 devices" }));
        assert!(matches!(refused, Err(SubmitError::Refused(ref m)) if m == "no machine fits 8 devices"), "{refused:?}");
    }

    #[test]
    fn a_dispatch_failure_carries_the_recorded_job() {
        let job = json!({ "id": "job-7", "status": "failed", "error": "ssh refused" });
        match submit_answer(422, json!({ "error": "dispatch failed", "job": job })) {
            Err(SubmitError::Dispatch { message, job }) => {
                assert_eq!(message, "dispatch failed");
                assert_eq!(job.id, "job-7");
            }
            other => panic!("expected a dispatch failure, got {other:?}"),
        }
    }

    #[test]
    fn a_refusal_without_a_status_text_names_the_status() {
        assert!(matches!(submit_answer(403, json!({})), Err(SubmitError::Refused(ref m)) if m == "HTTP 403"));
    }
}
