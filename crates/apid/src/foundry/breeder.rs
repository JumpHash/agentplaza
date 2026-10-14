//! The breeder's tools and `invoke_model` (`docs/codex-draft/playbooks/breeding-guide.md`, S-006).
//!
//! The job and dataset tools run the same handlers as the Foundry's HTTP routes (`foundry::routes`),
//! so the breeder and the routes share one path and one set of checks. `invoke_model`
//! and `foundry_request_shadow` act on the model registry, which is built. The laws (`statutes.jhrl`)
//! decide who may call each tool, and the model card's `allowed_roles` decides who may run a model.

use std::time::Instant;

use apz_proto::chat::ToolSpec;
use serde_json::{Value, json};

use crate::agents::Agent;
use crate::foundry::datasets::ExportSpec;
use crate::foundry::hf;
use crate::foundry::routes;
use crate::prices::Tokens;
use crate::state::App;

/// The tools this module serves. The laws name them in S-006 and `tools.jhrl`.
pub const TOOLS: &[&str] = &[
    "foundry_datasets",
    "foundry_export",
    "foundry_submit_job",
    "foundry_job_status",
    "foundry_request_shadow",
    "invoke_model",
    "foundry_hf_search",
];

/// The model's usage is metered under this name: `model:<id>`, cost 0.
pub fn model_usage_name(id: &str) -> String {
    format!("model:{id}")
}

/// The spec of one of [`TOOLS`], or `None` for any other name.
pub fn spec(name: &str) -> Option<ToolSpec> {
    let (description, input_schema) = match name {
        "foundry_datasets" => (
            "List the Foundry's datasets: exports and uploads with provenance, time range, label balance and taint flags.",
            json!({ "type": "object", "properties": {} }),
        ),
        "foundry_export" => (
            "Export plaza records (cascade outcomes, tasks, usage, or all) as a dataset for training. Returns the dataset.",
            json!({
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "enum": ["cascade", "tasks", "usage", "all"] },
                    "name": { "type": "string" },
                    "since_ms": { "type": "integer" },
                    "until_ms": { "type": "integer" }
                },
                "required": ["kind", "name"]
            }),
        ),
        "foundry_submit_job" => (
            "Submit a training job for a vetted recipe. The laws cap a breeder at 8 GPU-hours (budget_gpu_hours).",
            json!({ "type": "object", "properties": { "job": { "type": "object" } }, "required": ["job"] }),
        ),
        "foundry_job_status" => (
            "Read a training job's state, metrics and events.",
            json!({ "type": "object", "properties": { "job": { "type": "string" } }, "required": ["job"] }),
        ),
        "foundry_request_shadow" => (
            "Put a trained model in shadow mode: it runs beside the path it would replace and its decisions are logged, not used.",
            json!({ "type": "object", "properties": { "model": { "type": "string" } }, "required": ["model"] }),
        ),
        "invoke_model" => (
            "Run a shadow or promoted model on an input object. The input and output are checked against the model card's schemas.",
            json!({
                "type": "object",
                "properties": { "model": { "type": "string" }, "input": { "type": "object" } },
                "required": ["model", "input"]
            }),
        ),
        "foundry_hf_search" => (
            "Search the public Hugging Face hub, read-only: models with their task, library, downloads, likes, gated and license. Imports stay the owner's.",
            json!({
                "type": "object",
                "properties": {
                    "search": { "type": "string" },
                    "task": { "type": "string" },
                    "library": { "type": "string" },
                    "sort": { "type": "string", "enum": ["downloads", "likes", "lastModified", "trending"] },
                    "limit": { "type": "integer" },
                    "cursor": { "type": "string" }
                }
            }),
        ),
        _ => return None,
    };
    Some(ToolSpec {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
    })
}

/// Runs one of [`TOOLS`] for `agent`. The caller has checked the laws already.
pub fn run(app: &App, agent: &Agent, name: &str, input: &Value) -> Result<String, String> {
    match name {
        "foundry_hf_search" => hf_search(app, input),
        "foundry_request_shadow" => {
            let id = str_arg(input, "model")?;
            let record = app.models.shadow(&id)?;
            Ok(json!({ "id": record.card.id, "state": record.state.name() }).to_string())
        }
        "invoke_model" => {
            let id = str_arg(input, "model")?;
            let payload = input.get("input").cloned().unwrap_or(Value::Null);
            let (output, latency_ms) = invoke(app, &id, &payload)?;
            crate::usage::meter(
                app,
                agent.cid,
                &model_usage_name(&id),
                Tokens::default(),
                Some(0.0),
                None,
                "invoke_model",
            );
            app.bus.publish(
                "model.invoke",
                &json!({ "model": id, "cid": agent.cid, "latency_ms": latency_ms }),
            );
            Ok(json!({ "model": id, "output": output, "latency_ms": latency_ms }).to_string())
        }
        "foundry_datasets" => {
            let list = app
                .foundry
                .read(|s| s.datasets.values().cloned().collect::<Vec<_>>());
            serde_json::to_string(&list).map_err(|err| err.to_string())
        }
        "foundry_export" => {
            let spec = export_spec(input)?;
            let dataset = routes::export_dataset(app, &spec)?;
            serde_json::to_string(&dataset).map_err(|err| err.to_string())
        }
        "foundry_submit_job" => {
            let doc = job_doc(input, agent.cid)?;
            let job = routes::submit_job(app, doc).map_err(|(_, msg)| msg)?;
            if let Some(msg) = &job.error {
                return Err(format!("job {} did not dispatch: {msg}", job.id));
            }
            Ok(routes::job_view(&job).to_string())
        }
        "foundry_job_status" => {
            let id = str_arg(input, "job")?;
            let job = app
                .foundry
                .read(|s| s.jobs.get(&id).cloned())
                .ok_or_else(|| format!("no job {id}"))?;
            Ok(routes::job_view(&job).to_string())
        }
        other => Err(format!("{other} is not a Foundry breeder tool")),
    }
}

/// Runs a shadow or promoted model on `input` and returns its output and latency in ms.
pub(crate) fn invoke(app: &App, id: &str, input: &Value) -> Result<(Value, u128), String> {
    let record = app.models.get(id).ok_or_else(|| format!("no model {id}"))?;
    if !matches!(
        record.state,
        crate::foundry::registry::State::Shadow | crate::foundry::registry::State::Promoted
    ) {
        return Err(format!(
            "model {id} is {}: only shadow and promoted models run",
            record.state.name()
        ));
    }
    let started = Instant::now();
    if let Some(text) = app.models.text(id) {
        return Ok((text.invoke(input)?, started.elapsed().as_millis()));
    }
    let model = app
        .models
        .loaded(id)
        .ok_or_else(|| format!("model {id} is not loaded"))?;
    let output = model.invoke(input)?;
    Ok((output, started.elapsed().as_millis()))
}

/// The shadow hook: the path a model would replace calls this with the model's input and the real
/// decision it made with its cost. The model runs and its decision is compared with the real one
/// (same `label` when both have one, else the same whole output); the real decision is what
/// happens. Returns whether they agreed.
pub fn shadow_run(
    app: &App,
    id: &str,
    input: &Value,
    real: &Value,
    real_cost_usd: f64,
) -> Result<bool, String> {
    let record = app.models.get(id).ok_or_else(|| format!("no model {id}"))?;
    if record.state != crate::foundry::registry::State::Shadow {
        return Err(format!(
            "model {id} is {}: shadow runs are for shadow models",
            record.state.name()
        ));
    }
    let (output, _) = invoke(app, id, input)?;
    let agreed = decision(&output) == decision(real);
    app.models.record_shadow(id, agreed, real_cost_usd)?;
    Ok(agreed)
}

fn decision(value: &Value) -> Value {
    value.get("label").cloned().unwrap_or_else(|| value.clone())
}

/// The export a breeder asks for: the fields the export route takes (`kind`, `name`, the time range).
fn export_spec(input: &Value) -> Result<ExportSpec, String> {
    str_arg(input, "kind")?;
    str_arg(input, "name")?;
    serde_json::from_value(input.clone()).map_err(|err| format!("export spec: {err}"))
}

/// The job document a breeder submits: its `job` object, with the breeder's own cid as `agent`, so
/// the laws judge the breeder's role rather than whatever the document claims.
fn job_doc(input: &Value, cid: u64) -> Result<Value, String> {
    let mut doc = input
        .get("job")
        .filter(|job| job.is_object())
        .cloned()
        .ok_or_else(|| "job is required (an object)".to_string())?;
    doc["agent"] = json!(cid);
    Ok(doc)
}

fn str_arg(input: &Value, key: &str) -> Result<String, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{key} is required (a string)"))
}

/// The GPU-hours a job asks for, rounded up to whole hours for the laws (a missing or
/// unreadable budget is the largest value, so the law refuses it).
pub fn budget_hours(input: &Value) -> i128 {
    input
        .get("job")
        .and_then(|job| job.get("budget_gpu_hours"))
        .and_then(Value::as_f64)
        .filter(|hours| hours.is_finite() && *hours >= 0.0)
        .map_or(i128::MAX, |hours| hours.ceil() as i128)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foundry::jobs;

    fn parsed(job: Value) -> Result<jobs::Spec, (u16, String)> {
        let doc = job_doc(&json!({ "job": job }), 7).unwrap();
        jobs::parse(&doc, || "tj-test".to_string())
    }

    #[test]
    fn the_job_and_dataset_tools_have_specs() {
        for name in [
            "foundry_datasets",
            "foundry_export",
            "foundry_submit_job",
            "foundry_job_status",
        ] {
            assert!(spec(name).is_some(), "{name} has no spec");
        }
    }

    #[test]
    fn an_export_needs_its_kind_and_name() {
        let err = export_spec(&json!({ "source": "cascade" })).err().unwrap();
        assert!(err.starts_with("kind is required"), "{err}");
        let ok = export_spec(&json!({ "kind": "cascade", "name": "c1" }))
            .ok()
            .unwrap();
        assert_eq!(ok.kind, "cascade");
    }

    #[test]
    fn a_submitted_job_runs_under_the_breeders_cid() {
        assert!(job_doc(&json!({}), 7).is_err());
        assert!(job_doc(&json!({ "job": "tj-1" }), 7).is_err());
        let doc = job_doc(&json!({ "job": { "agent": 99, "recipe": "r@1" } }), 7).unwrap();
        assert_eq!(doc["agent"], json!(7));
    }

    #[test]
    fn a_job_over_the_gpu_hour_cap_is_refused() {
        let over =
            parsed(json!({ "recipe": "r@1", "datasets": ["ds-1"], "budget_gpu_hours": 8.5 }))
                .unwrap();
        let (status, msg) = jobs::budget_check(&over).unwrap_err();
        assert_eq!(status, 403);
        assert!(msg.contains("GPU-hour cap"), "{msg}");
        let at_cap =
            parsed(json!({ "recipe": "r@1", "datasets": ["ds-1"], "budget_gpu_hours": 8.0 }))
                .unwrap();
        assert!(jobs::budget_check(&at_cap).is_ok());
    }
}

/// The breeder's `foundry_hf_search`: a read-only search of the Hugging Face hub, in the route's compact shape.
/// Imports are not a breeder's: they stay owner-only.
fn hf_search(app: &App, input: &Value) -> Result<String, String> {
    let query = search_query(input)?;
    let block = app.settings.get().fetch_block;
    hf::search_page(&query, &block)
        .map(|page| page.to_string())
        .map_err(|fail| fail.message)
}

/// The search the input asks for, with the same fields and limit rules as the route.
fn search_query(input: &Value) -> Result<hf::Search, String> {
    let text = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let limit = match input.get("limit") {
        None | Some(Value::Null) => hf::DEFAULT_LIMIT,
        Some(value) => value
            .as_u64()
            .filter(|n| (1..=100).contains(n))
            .ok_or("limit must be a whole number from 1 to 100")? as usize,
    };
    Ok(hf::Search {
        search: text("search"),
        task: text("task"),
        library: text("library"),
        sort: text("sort"),
        limit,
        cursor: Some(text("cursor")).filter(|c| !c.is_empty()),
    })
}

#[cfg(test)]
mod hf_search_tests {
    use super::*;

    #[test]
    fn the_breeder_has_a_read_only_hub_search_and_no_import() {
        assert!(TOOLS.contains(&"foundry_hf_search"));
        assert!(spec("foundry_hf_search").is_some());
        assert!(
            !TOOLS.iter().any(|tool| tool.contains("import")),
            "imports stay owner-only"
        );
        assert_eq!(crate::laws::risk_class("foundry_hf_search"), "read");
        assert!(matches!(
            crate::tools::runs("foundry_hf_search"),
            Some(crate::tools::Runs::Host)
        ));
    }

    #[test]
    fn the_roles_manifest_gives_the_breeder_the_search() {
        let manifest: Value =
            serde_json::from_str(include_str!("../../../../docs/agents/initial-roles.json"))
                .unwrap();
        let tools = manifest["roles"]["breeder"]["tools"]
            .as_array()
            .expect("the breeder's tools");
        assert!(tools.iter().any(|tool| tool == "foundry_hf_search"));
    }

    #[test]
    fn the_search_input_is_checked_like_the_route() {
        let query =
            search_query(&json!({ "search": "sst2 onnx", "task": "text-classification" })).unwrap();
        assert_eq!(query.limit, hf::DEFAULT_LIMIT);
        assert_eq!(query.search, "sst2 onnx");
        assert_eq!(query.task, "text-classification");
        assert!(search_query(&json!({ "limit": 0 })).is_err());
        assert!(search_query(&json!({ "limit": 101 })).is_err());
        assert!(search_query(&json!({ "limit": "ten" })).is_err());
    }
}
