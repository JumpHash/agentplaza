//! Training datasets for the Foundry's rung predictor, built from the plaza's own records.
//! The row shape is the trainer's (`trainer/recipes/rung_predictor/recipe.py`): one JSON object
//! per line with `ts`, six numeric features, four categorical features and `label`, plus an
//! optional `task_id`. Prompts, task goals and secrets are never copied into a row.
//!
//! Sources read by [`export`]:
//! - the task ledger (`app.plaza.tasks`): state, role, claiming CID, created and updated times;
//! - the usage ledger (`app.usage`): per model call, its model, CID, role, task and input tokens;
//! - the transcripts (`app.transcripts`): per work item, the task it was charged to (the
//!   `work_start` origin), its prompt length, and its first model request's tool and message
//!   counts. Only the in-memory window is read (the newest entries per agent);
//!
//! Kinds: `tasks` (one row per task that finished `done`, labelled by the model that served its
//! last call), `cascade` (the `tasks` rows whose calls ran on two or more rungs, so the cascade
//! escalated; the label is the rung that finished the task), `usage` (one row per call charged
//! to a finished task, labelled by that call's model), and `all` (the same rows as `tasks`; usage
//! rows would duplicate them). See [`rung_for_price`] for the rung.
//!
//! Hygiene flags (the dataset's `taint`), and which of them block a job ([`blocking`]):
//! - `empty` (blocking): no training rows.
//! - `single_class` (blocking): the training rows carry fewer than two classes.
//! - `too_few_rows` (not blocking): 1 to 199 training rows.
//! - `label_skew` (not blocking): one class is more than 95% of the training rows.
//! - `unverified_upload` (blocking): an upload has columns the format does not know.
//!
//! Uploads (`kind` "upload") are JSONL files in the same row shape. A line must be a JSON object
//! with `ts` (a unix millisecond number), `label` (a class name or 0..2), all six numeric features
//! as whole numbers, and all four categorical features as strings; an optional `task_id` is a
//! string. Anything else rejects the file, with the line number. Extra keys are kept out of the
//! row and flag the upload `unverified_upload`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::settings::{Tier, tier_of};
use crate::state::App;
use crate::usage::CALLS_LIMIT;

/// Uploads over this many bytes are refused.
pub const MAX_UPLOAD_BYTES: usize = 50 * 1024 * 1024;
/// Uploads over this many non-empty lines are refused.
pub const MAX_UPLOAD_ROWS: usize = 2_000_000;
/// A row is frozen (held out for evaluation) when its hash bucket, 0..100, is below this.
pub const FROZEN_PERCENT: u64 = 10;
/// Fewer training rows than this is flagged `too_few_rows`.
pub const TOO_FEW_ROWS: u64 = 200;
/// A class holding more than this share of the training rows is flagged `label_skew`.
pub const SKEW_SHARE: f64 = 0.95;
/// Rung cut points, in USD per million output tokens: below the first is `cheap`, below the second
/// `mid`, otherwise `frontier`. These are assumptions for the owner to confirm; the records name
/// no rungs.
pub const CHEAP_BELOW_USD_PER_MTOK: f64 = 2.0;
pub const MID_BELOW_USD_PER_MTOK: f64 = 15.0;
/// The value of a categorical feature the records do not name.
pub const MISSING: &str = "__missing__";
pub const CLASSES: [&str; 3] = ["cheap", "mid", "frontier"];
const NUMERIC: [&str; 6] = [
    "prompt_chars",
    "prompt_tokens",
    "tool_count",
    "history_turns",
    "attachments",
    "prior_failures",
];
const CATEGORICAL: [&str; 4] = ["task_kind", "domain", "tenant_tier", "requested_model"];

/// One dataset: the training file (`file`), its held-out rows (`frozen_file`), and the hygiene
/// findings. Returned by [`export`] and [`upload`]; the parent stores it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, apz_store::Table)]
pub struct Dataset {
    /// "ds-" + first 16 hex chars of the sha256 of the JSONL file bytes.
    pub id: String,
    pub name: String,
    /// "cascade" | "tasks" | "usage" | "upload" (an export of kind "all" is recorded as "all").
    pub kind: String,
    pub created_ms: u64,
    /// The time range of the rows (min ts), 0 when there are none.
    pub from_ms: u64,
    /// The time range of the rows (max ts).
    pub to_ms: u64,
    /// Training rows (the frozen rows are not counted here).
    pub rows: u64,
    pub frozen_rows: u64,
    /// Training rows per class name.
    #[table(map_value)]
    pub label_balance: BTreeMap<String, u64>,
    /// Hygiene flags, sorted and deduplicated.
    #[table(list)]
    pub taint: Vec<String>,
    /// Where the rows came from: sources, counts, filters, the export spec and field notes (shown verbatim).
    #[table(content)]
    pub provenance: Value,
    /// Path relative to the foundry data dir, e.g. "datasets/ds-xxxx.jsonl".
    pub file: String,
    /// e.g. "datasets/ds-xxxx.frozen.jsonl".
    pub frozen_file: String,
    /// Full hex sha256 of the training file.
    pub sha256: String,
}

/// What to export. `kind` is "cascade" | "tasks" | "usage" | "all"; `since_ms` is inclusive and
/// `until_ms` exclusive, both on a row's ts.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ExportSpec {
    pub kind: String,
    pub name: String,
    pub since_ms: Option<u64>,
    pub until_ms: Option<u64>,
}

/// A task as the ledger holds it (plain fields, so the mapping is testable without an [`App`]).
#[derive(Debug, Clone, PartialEq)]
pub struct TaskRec {
    pub id: String,
    /// The role the task was created for.
    pub role: Option<String>,
    /// "open" | "claimed" | "done" | "failed" | "withdrawn".
    pub state: String,
    /// The claiming agent's CID, when one claimed it.
    pub agent_cid: Option<u64>,
    pub created_ms: u64,
    pub updated_ms: u64,
}

/// A usage call. `prompt_tokens` is input + cache read + cache write, the prompt the model read.
#[derive(Debug, Clone, PartialEq)]
pub struct CallRec {
    pub id: u64,
    pub at_ms: u64,
    pub cid: u64,
    pub role: String,
    pub model: String,
    pub task: Option<String>,
    pub prompt_tokens: u64,
}

/// What a work item's transcript says: the task it was charged to, its prompt length, and the tool
/// and message counts of its first model request (`requests` counts the model requests seen).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorkRec {
    pub cid: u64,
    pub work: u64,
    pub task: Option<String>,
    pub prompt_chars: u64,
    pub tools: u64,
    pub messages: u64,
    pub requests: u64,
}

/// The features of one row. Categorical values are `MISSING` when unknown.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Features {
    pub prompt_chars: u64,
    pub prompt_tokens: u64,
    pub tool_count: u64,
    pub history_turns: u64,
    pub attachments: u64,
    pub prior_failures: u64,
    pub task_kind: String,
    pub domain: String,
    pub tenant_tier: String,
    pub requested_model: String,
}

/// One candidate row, ready for the split and the files.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub task_id: Option<String>,
    pub ts: u64,
    /// The row as written (the trainer's shape).
    pub row: Value,
    /// The class name.
    pub label: String,
}

/// The gathered rows, and the counts of what was kept and what was skipped and why.
#[derive(Debug, Clone, Default)]
pub struct Gathered {
    pub items: Vec<Item>,
    pub counts: BTreeMap<String, u64>,
}

/// A built dataset and the bytes of its two files.
#[derive(Debug, Clone)]
pub struct Assembled {
    pub dataset: Dataset,
    pub training: Vec<u8>,
    pub frozen: Vec<u8>,
}

/// Exports the plaza's records into a dataset under `data_dir` (the parent passes <state>/foundry).
pub fn export(app: &App, data_dir: &Path, spec: &ExportSpec) -> Result<Dataset, String> {
    if !matches!(spec.kind.as_str(), "cascade" | "tasks" | "usage" | "all") {
        return Err(format!(
            "unknown dataset kind {:?}; use cascade, tasks, usage or all",
            spec.kind
        ));
    }
    let now = apz_plaza::now_ms();
    let tasks: Vec<TaskRec> = app
        .plaza
        .tasks
        .list()
        .iter()
        .map(|task| {
            let view = task.view();
            TaskRec {
                id: view.id.to_string(),
                role: view.role.map(String::from),
                state: view.state.to_string(),
                agent_cid: view.agent_cid,
                created_ms: view.created_at,
                updated_ms: view.updated_at,
            }
        })
        .collect();
    let mut calls = Vec::new();
    let mut before = None;
    loop {
        let page = app.usage.calls(before, CALLS_LIMIT, None, None);
        calls.extend(page.calls.iter().map(|c| CallRec {
            id: c.id,
            at_ms: c.at_ms,
            cid: c.cid,
            role: c.role.clone(),
            model: c.model.clone(),
            task: c.task.clone(),
            prompt_tokens: c.tokens.input + c.tokens.cache_read + c.tokens.cache_write,
        }));
        match page.next_before {
            Some(next) => before = Some(next),
            None => break,
        }
    }
    let cids: BTreeSet<u64> = calls
        .iter()
        .map(|c| c.cid)
        .chain(tasks.iter().filter_map(|t| t.agent_cid))
        .collect();
    let mut entries: Vec<(u64, u64, String, Value)> = Vec::new();
    for cid in &cids {
        for entry in app.transcripts.after(*cid, 0, usize::MAX) {
            let json = serde_json::to_value(&entry).unwrap_or(serde_json::Value::Null);
            let kind = json["kind"].as_str().unwrap_or_default().to_string();
            entries.push((entry.cid, entry.work, kind, json["data"].clone()));
        }
    }
    let refs: Vec<(u64, u64, &str, &Value)> = entries
        .iter()
        .map(|(c, w, k, d)| (*c, *w, k.as_str(), d))
        .collect();
    let works = works_from(&refs);
    let manual = app.settings.get().prices;
    let rung =
        |model: &str| rung_for_price(app.prices.lookup(model, &manual).map(|p| p.price.output));
    let gathered = gather(
        &spec.kind,
        &tasks,
        &calls,
        &works,
        &rung,
        spec.since_ms,
        spec.until_ms,
    );
    let name = if spec.name.trim().is_empty() {
        format!("{}-{now}", spec.kind)
    } else {
        spec.name.trim().to_string()
    };
    let provenance = json!({
        "source": "plaza",
        "spec": { "kind": spec.kind, "since_ms": spec.since_ms, "until_ms": spec.until_ms },
        "sources": {
            "tasks": tasks.len(),
            "usage_calls": calls.len(),
            "transcript_works": works.len(),
            "transcript_agents": cids.len(),
        },
        "counts": gathered.counts,
        "fields": {
            "tasks": ["id", "role", "state", "agent_cid", "created_at", "updated_at"],
            "usage": ["id", "at", "cid", "role", "model", "task", "tokens.input", "tokens.cache_read", "tokens.cache_write"],
            "transcript": ["work_start.origin.task", "work_start.prompt (length only)", "model_request.tools (count)", "model_request.messages (count)"],
        },
        "features": {
            "prompt_chars": "length of the work_start prompt of the task's work; 0 when the transcript window has no such work",
            "prompt_tokens": "input + cache read + cache write of the task's first usage call (a usage row: its own call); 0 when none",
            "tool_count": "tools in the work's first model request; 0 when unknown",
            "history_turns": "messages in the work's first model request; 0 when unknown",
            "attachments": "constant 0: no record counts attachments",
            "prior_failures": "failed tasks with the same role whose updated_at is at or before this task's created_at",
            "task_kind": "the task's role, or __missing__",
            "domain": "constant __missing__: no record names a domain",
            "tenant_tier": "the calling role's settings tier (orchestrator or worker), or __missing__",
            "requested_model": "the model of the task's last usage call (a usage row: the call's model), or __missing__",
        },
        "ts": "the task's created_at (a usage row: the call's at)",
        "label": {
            "rung": "the rung of the model that served the last call, by its catalog output price read at export time (USD per million tokens)",
            "cut_usd_per_mtok": { "cheap_below": CHEAP_BELOW_USD_PER_MTOK, "mid_below": MID_BELOW_USD_PER_MTOK },
            "passed": "the task's state is done; failed and withdrawn tasks have no passing rung and give no rows",
            "limit": "the lowest passing rung is not recorded; the rung of the model that finished the task is used",
        },
        "split": "a task_id is frozen when the first 8 bytes of its sha256, as a big-endian u64, mod 100 is below 10; a row without task_id uses the sha256 of its JSON",
        "window": "since_ms <= ts < until_ms",
    });
    let assembled = assemble(&name, &spec.kind, now, gathered.items, false, provenance);
    store(data_dir, &assembled)?;
    Ok(assembled.dataset)
}

/// Stores an owner upload: `jsonl` is the file's bytes (one row per line, the same row shape as
/// exports); rows are validated and hygiene-checked the same way.
pub fn upload(data_dir: &Path, name: &str, jsonl: &[u8], now_ms: u64) -> Result<Dataset, String> {
    if jsonl.len() > MAX_UPLOAD_BYTES {
        return Err(format!(
            "upload is {} bytes; the limit is {MAX_UPLOAD_BYTES}",
            jsonl.len()
        ));
    }
    let text = std::str::from_utf8(jsonl).map_err(|_| "upload is not UTF-8".to_string())?;
    let lines: Vec<(usize, &str)> = text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .collect();
    if lines.len() > MAX_UPLOAD_ROWS {
        return Err(format!(
            "upload has {} rows; the limit is {MAX_UPLOAD_ROWS}",
            lines.len()
        ));
    }
    let mut items = Vec::with_capacity(lines.len());
    let mut unknown = false;
    for (number, line) in lines {
        let (item, unknown_line) =
            parse_upload_line(line).map_err(|e| format!("line {}: {e}", number + 1))?;
        unknown |= unknown_line;
        items.push(item);
    }
    let provenance = json!({
        "source": "upload",
        "bytes": jsonl.len(),
        "lines": items.len(),
        "unknown_columns": unknown,
    });
    let name = if name.trim().is_empty() {
        format!("upload-{now_ms}")
    } else {
        name.trim().to_string()
    };
    let assembled = assemble(&name, "upload", now_ms, items, unknown, provenance);
    store(data_dir, &assembled)?;
    Ok(assembled.dataset)
}

/// The hygiene flags that stop a job from using a dataset. Anything in this list blocks.
pub fn blocking(flag: &str) -> bool {
    matches!(flag, "empty" | "single_class" | "unverified_upload")
}

/// The rung of a model from its catalog output price (USD per million tokens); `None` when no
/// price is known. Cut points: [`CHEAP_BELOW_USD_PER_MTOK`], [`MID_BELOW_USD_PER_MTOK`].
pub fn rung_for_price(output_usd_per_mtok: Option<f64>) -> Option<&'static str> {
    let price = output_usd_per_mtok.filter(|p| p.is_finite() && *p >= 0.0)?;
    Some(if price < CHEAP_BELOW_USD_PER_MTOK {
        "cheap"
    } else if price < MID_BELOW_USD_PER_MTOK {
        "mid"
    } else {
        "frontier"
    })
}

/// The row for one record: the trainer's shape, with the label as a class name.
pub fn row_value(task_id: Option<&str>, ts: u64, f: &Features, label: &str) -> Value {
    let mut row = json!({
        "ts": ts,
        "prompt_chars": f.prompt_chars,
        "prompt_tokens": f.prompt_tokens,
        "tool_count": f.tool_count,
        "history_turns": f.history_turns,
        "attachments": f.attachments,
        "prior_failures": f.prior_failures,
        "task_kind": f.task_kind,
        "domain": f.domain,
        "tenant_tier": f.tenant_tier,
        "requested_model": f.requested_model,
        "label": label,
    });
    if let Some(id) = task_id {
        row["task_id"] = json!(id);
    }
    row
}

/// The transcript's work records, one per (agent, work). `entries` are (cid, work, kind, data) in
/// the order they were recorded. Only `work_start` (task origin, prompt length) and the first
/// `model_request` (tool and message counts) are read; no text is kept.
pub fn works_from(entries: &[(u64, u64, &str, &Value)]) -> Vec<WorkRec> {
    let mut works: BTreeMap<(u64, u64), WorkRec> = BTreeMap::new();
    for &(cid, work, kind, data) in entries {
        let rec = works.entry((cid, work)).or_insert_with(|| WorkRec {
            cid,
            work,
            ..WorkRec::default()
        });
        match kind {
            "work_start" => {
                rec.task = data
                    .get("origin")
                    .and_then(|o| o.get("task"))
                    .and_then(Value::as_str)
                    .map(String::from);
                rec.prompt_chars = data
                    .get("prompt")
                    .and_then(Value::as_str)
                    .map_or(0, |p| p.chars().count() as u64);
            }
            "model_request" => {
                if rec.requests == 0 {
                    rec.tools = data
                        .get("tools")
                        .and_then(Value::as_array)
                        .map_or(0, |t| t.len() as u64);
                    rec.messages = data.get("messages").and_then(Value::as_u64).unwrap_or(0);
                }
                rec.requests += 1;
            }
            _ => {}
        }
    }
    works.into_values().collect()
}

/// Maps the plaza's records to candidate rows (the pure core of [`export`]).
pub fn gather(
    kind: &str,
    tasks: &[TaskRec],
    calls: &[CallRec],
    works: &[WorkRec],
    rung: &dyn Fn(&str) -> Option<&'static str>,
    since: Option<u64>,
    until: Option<u64>,
) -> Gathered {
    let mut out = Gathered::default();
    let mut calls_by_task: BTreeMap<&str, Vec<&CallRec>> = BTreeMap::new();
    for call in calls {
        if let Some(task) = &call.task {
            calls_by_task.entry(task.as_str()).or_default().push(call);
        }
    }
    for list in calls_by_task.values_mut() {
        list.sort_by_key(|c| (c.at_ms, c.id));
    }
    let mut ordered: Vec<&WorkRec> = works.iter().collect();
    ordered.sort_by_key(|w| (w.cid, w.work));
    let mut work_by_task: BTreeMap<&str, &WorkRec> = BTreeMap::new();
    for work in ordered {
        if let Some(task) = &work.task {
            work_by_task.entry(task.as_str()).or_insert(work);
        }
    }
    let mut sorted: Vec<&TaskRec> = tasks.iter().collect();
    sorted.sort_by(|a, b| (a.created_ms, &a.id).cmp(&(b.created_ms, &b.id)));

    for task in sorted {
        bump(&mut out.counts, &format!("state.{}", task.state));
        if task.state != "done" {
            continue;
        }
        let Some(list) = calls_by_task.get(task.id.as_str()) else {
            bump(&mut out.counts, "no_model_call");
            continue;
        };
        let Some(&last) = list.last() else {
            bump(&mut out.counts, "no_model_call");
            continue;
        };
        if kind == "cascade" {
            let rungs: BTreeSet<&str> = list.iter().filter_map(|c| rung(&c.model)).collect();
            if rungs.len() < 2 {
                bump(&mut out.counts, "single_rung");
                continue;
            }
        }
        let work = work_by_task.get(task.id.as_str()).copied();
        let base = Features {
            prompt_chars: work.map_or(0, |w| w.prompt_chars),
            tool_count: work.map_or(0, |w| w.tools),
            history_turns: work.map_or(0, |w| w.messages),
            attachments: 0,
            prior_failures: prior_failures(tasks, task),
            task_kind: task
                .role
                .clone()
                .filter(|r| !r.is_empty())
                .unwrap_or_else(|| MISSING.to_string()),
            domain: MISSING.to_string(),
            ..Features::default()
        };
        if kind == "usage" {
            for call in list {
                let Some(label) = rung(&call.model) else {
                    bump(&mut out.counts, "no_price");
                    continue;
                };
                let features = Features {
                    prompt_tokens: call.prompt_tokens,
                    tenant_tier: tier_name(&call.role),
                    requested_model: call.model.clone(),
                    ..base.clone()
                };
                push(
                    &mut out,
                    Some(&task.id),
                    call.at_ms,
                    features,
                    label,
                    since,
                    until,
                );
            }
        } else {
            let Some(label) = rung(&last.model) else {
                bump(&mut out.counts, "no_price");
                continue;
            };
            let features = Features {
                prompt_tokens: list[0].prompt_tokens,
                tenant_tier: tier_name(&last.role),
                requested_model: last.model.clone(),
                ..base
            };
            push(
                &mut out,
                Some(&task.id),
                task.created_ms,
                features,
                label,
                since,
                until,
            );
        }
    }
    out
}

/// Parses one upload line into a candidate row, and says whether it has columns the format does
/// not know (those are dropped from the row).
pub fn parse_upload_line(line: &str) -> Result<(Item, bool), String> {
    let value: Value = serde_json::from_str(line).map_err(|e| format!("not JSON: {e}"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| "not a JSON object".to_string())?;
    let ts = obj
        .get("ts")
        .and_then(Value::as_u64)
        .ok_or_else(|| "ts must be a unix millisecond number".to_string())?;
    let num = |key: &str| {
        obj.get(key)
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("{key} must be a whole number"))
    };
    let text = |key: &str| {
        obj.get(key)
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| format!("{key} must be a string"))
    };
    let features = Features {
        prompt_chars: num("prompt_chars")?,
        prompt_tokens: num("prompt_tokens")?,
        tool_count: num("tool_count")?,
        history_turns: num("history_turns")?,
        attachments: num("attachments")?,
        prior_failures: num("prior_failures")?,
        task_kind: text("task_kind")?,
        domain: text("domain")?,
        tenant_tier: text("tenant_tier")?,
        requested_model: text("requested_model")?,
    };
    let label = match obj.get("label") {
        Some(Value::String(s)) if CLASSES.contains(&s.as_str()) => s.clone(),
        Some(v) if v.as_u64().is_some_and(|n| n < 3) => {
            CLASSES[v.as_u64().unwrap_or(0) as usize].to_string()
        }
        _ => return Err("label must be cheap, mid, frontier, or 0, 1, 2".to_string()),
    };
    let task_id = match obj.get("task_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err("task_id must be a string".to_string()),
    };
    let known: BTreeSet<&str> = NUMERIC
        .iter()
        .chain(CATEGORICAL.iter())
        .copied()
        .chain(["ts", "label", "task_id"])
        .collect();
    let unknown = obj.keys().any(|k| !known.contains(k.as_str()));
    let row = row_value(task_id.as_deref(), ts, &features, &label);
    Ok((
        Item {
            task_id,
            ts,
            row,
            label,
        },
        unknown,
    ))
}

/// The training rows' class counts.
pub fn label_balance(training: &[&Item]) -> BTreeMap<String, u64> {
    let mut balance = BTreeMap::new();
    for item in training {
        *balance.entry(item.label.clone()).or_insert(0) += 1;
    }
    balance
}

/// The hygiene flags. `training` are the rows a job would read. `unknown_columns` is set for an upload
/// with unknown columns.
pub fn taint_flags(training: &[&Item], unknown_columns: bool) -> Vec<String> {
    let mut flags: BTreeSet<&str> = BTreeSet::new();
    let n = training.len() as u64;
    if n == 0 {
        flags.insert("empty");
    } else {
        if n < TOO_FEW_ROWS {
            flags.insert("too_few_rows");
        }
        let balance = label_balance(training);
        if balance.len() < 2 {
            flags.insert("single_class");
        }
        if balance.values().any(|&c| c as f64 > SKEW_SHARE * n as f64) {
            flags.insert("label_skew");
        }
    }
    if unknown_columns {
        flags.insert("unverified_upload");
    }
    flags.into_iter().map(String::from).collect()
}

/// The dataset id for these training bytes: "ds-" + the first 16 hex chars of their sha256.
pub fn dataset_id(training: &[u8]) -> String {
    format!("ds-{}", &hex(&sha256(training))[..16])
}

/// Whether a row is held out: by the hash of its task_id, or of its JSON when it has none.
pub fn is_frozen(task_id: Option<&str>, row: &Value) -> bool {
    let key = match task_id {
        Some(id) => id.as_bytes().to_vec(),
        None => row.to_string().into_bytes(),
    };
    let digest = sha256(&key);
    let bucket = u64::from_be_bytes(digest[..8].try_into().unwrap_or([0; 8])) % 100;
    bucket < FROZEN_PERCENT
}

/// Splits, sorts and flags the rows, and builds the two files' bytes (pure; [`store`] writes them).
pub fn assemble(
    name: &str,
    kind: &str,
    created_ms: u64,
    mut items: Vec<Item>,
    unknown_columns: bool,
    provenance: Value,
) -> Assembled {
    items.sort_by_cached_key(|i| (i.ts, i.task_id.clone(), i.row.to_string()));
    let held_mask: Vec<bool> = items
        .iter()
        .map(|i| is_frozen(i.task_id.as_deref(), &i.row))
        .collect();
    let training: Vec<&Item> = items
        .iter()
        .zip(&held_mask)
        .filter(|(_, held)| !**held)
        .map(|(i, _)| i)
        .collect();
    let held: Vec<&Item> = items
        .iter()
        .zip(&held_mask)
        .filter(|(_, held)| **held)
        .map(|(i, _)| i)
        .collect();
    let flags = taint_flags(&training, unknown_columns);
    let training_bytes = lines_of(&training);
    let frozen_bytes = lines_of(&held);
    let id = dataset_id(&training_bytes);
    let dataset = Dataset {
        id: id.clone(),
        name: name.to_string(),
        kind: kind.to_string(),
        created_ms,
        from_ms: items.iter().map(|i| i.ts).min().unwrap_or(0),
        to_ms: items.iter().map(|i| i.ts).max().unwrap_or(0),
        rows: training.len() as u64,
        frozen_rows: held.len() as u64,
        label_balance: label_balance(&training),
        taint: flags,
        provenance,
        file: format!("datasets/{id}.jsonl"),
        frozen_file: format!("datasets/{id}.frozen.jsonl"),
        sha256: hex(&sha256(&training_bytes)),
    };
    Assembled {
        dataset,
        training: training_bytes,
        frozen: frozen_bytes,
    }
}

/// Writes the two files under `data_dir/datasets`, each through a temporary file and a rename.
pub fn store(data_dir: &Path, assembled: &Assembled) -> Result<(), String> {
    let dir = data_dir.join("datasets");
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let id = &assembled.dataset.id;
    write_atomic(&dir.join(format!("{id}.jsonl")), &assembled.training)?;
    write_atomic(&dir.join(format!("{id}.frozen.jsonl")), &assembled.frozen)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("bad path {}", path.display()))?;
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let mut file = File::create(&tmp).map_err(|e| format!("create {}: {e}", tmp.display()))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("write {}: {e}", tmp.display()))?;
    drop(file);
    std::fs::rename(&tmp, path).map_err(|e| format!("rename to {}: {e}", path.display()))
}

fn lines_of(items: &[&Item]) -> Vec<u8> {
    let mut out = String::new();
    for item in items {
        out.push_str(&item.row.to_string());
        out.push('\n');
    }
    out.into_bytes()
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    Sha256::digest(bytes).to_vec()
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn bump(counts: &mut BTreeMap<String, u64>, key: &str) {
    *counts.entry(key.to_string()).or_insert(0) += 1;
}

fn tier_name(role: &str) -> String {
    if role.is_empty() {
        return MISSING.to_string();
    }
    match tier_of(role) {
        Tier::Orchestrator => "orchestrator".to_string(),
        Tier::Worker => "worker".to_string(),
    }
}

/// Failed tasks of the same role whose last update came before this task was created.
fn prior_failures(tasks: &[TaskRec], task: &TaskRec) -> u64 {
    tasks
        .iter()
        .filter(|u| {
            u.id != task.id
                && u.state == "failed"
                && u.role == task.role
                && u.updated_ms <= task.created_ms
        })
        .count() as u64
}

fn push(
    out: &mut Gathered,
    task_id: Option<&String>,
    ts: u64,
    features: Features,
    label: &str,
    since: Option<u64>,
    until: Option<u64>,
) {
    if since.is_some_and(|s| ts < s) || until.is_some_and(|u| ts >= u) {
        bump(&mut out.counts, "outside_window");
        return;
    }
    let task_id = task_id.cloned();
    let row = row_value(task_id.as_deref(), ts, &features, label);
    out.items.push(Item {
        task_id,
        ts,
        row,
        label: label.to_string(),
    });
    bump(&mut out.counts, "rows");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn features(task_kind: &str, requested_model: &str) -> Features {
        Features {
            task_kind: task_kind.into(),
            domain: MISSING.into(),
            tenant_tier: "worker".into(),
            requested_model: requested_model.into(),
            ..Features::default()
        }
    }

    fn item(task: &str, ts: u64, label: &str) -> Item {
        let row = row_value(Some(task), ts, &features("dev", "m"), label);
        Item {
            task_id: Some(task.into()),
            ts,
            row,
            label: label.into(),
        }
    }

    fn task(id: &str, state: &str, created: u64) -> TaskRec {
        TaskRec {
            id: id.into(),
            role: Some("coder".into()),
            state: state.into(),
            agent_cid: Some(3),
            created_ms: created,
            updated_ms: created + 1_000,
        }
    }

    fn call(id: u64, at: u64, model: &str, task: &str) -> CallRec {
        CallRec {
            id,
            at_ms: at,
            cid: 3,
            role: "coder".into(),
            model: model.into(),
            task: Some(task.into()),
            prompt_tokens: 420,
        }
    }

    fn rung_map(model: &str) -> Option<&'static str> {
        match model {
            "cheap-model" => Some("cheap"),
            "frontier-model" => Some("frontier"),
            _ => None,
        }
    }

    #[test]
    fn a_finished_task_maps_to_one_row_in_the_trainer_shape() {
        let tasks = vec![task("t7", "done", 1_000)];
        let calls = vec![call(11, 1_100, "cheap-model", "t7")];
        let works = vec![WorkRec {
            cid: 3,
            work: 5,
            task: Some("t7".into()),
            prompt_chars: 1_200,
            tools: 4,
            messages: 2,
            requests: 1,
        }];
        let g = gather("tasks", &tasks, &calls, &works, &rung_map, None, None);
        assert_eq!(g.items.len(), 1);
        assert_eq!(
            g.items[0].row,
            json!({
                "task_id": "t7", "ts": 1000, "prompt_chars": 1200, "prompt_tokens": 420, "tool_count": 4,
                "history_turns": 2, "attachments": 0, "prior_failures": 0, "task_kind": "coder",
                "domain": "__missing__", "tenant_tier": "worker", "requested_model": "cheap-model", "label": "cheap",
            })
        );
    }

    #[test]
    fn failed_tasks_give_no_rows_and_are_counted() {
        let tasks = vec![task("t1", "failed", 1_000)];
        let calls = vec![call(1, 1_100, "cheap-model", "t1")];
        let g = gather("tasks", &tasks, &calls, &[], &rung_map, None, None);
        assert!(g.items.is_empty());
        assert_eq!(g.counts.get("state.failed"), Some(&1));
    }

    #[test]
    fn cascade_keeps_only_tasks_that_escalated_and_labels_the_passing_rung() {
        let tasks = vec![task("t1", "done", 1_000), task("t2", "done", 2_000)];
        let calls = vec![
            call(1, 1_100, "cheap-model", "t1"),
            call(2, 1_200, "frontier-model", "t1"),
            call(3, 2_100, "cheap-model", "t2"),
        ];
        let g = gather("cascade", &tasks, &calls, &[], &rung_map, None, None);
        assert_eq!(g.items.len(), 1);
        assert_eq!(g.items[0].task_id.as_deref(), Some("t1"));
        assert_eq!(g.items[0].label, "frontier");
        assert_eq!(g.counts.get("single_rung"), Some(&1));
    }

    #[test]
    fn usage_rows_are_per_call() {
        let tasks = vec![task("t1", "done", 1_000)];
        let calls = vec![
            call(1, 1_100, "cheap-model", "t1"),
            call(2, 1_200, "frontier-model", "t1"),
        ];
        let g = gather("usage", &tasks, &calls, &[], &rung_map, None, None);
        assert_eq!(g.items.len(), 2);
        assert_eq!(g.items[1].label, "frontier");
    }

    #[test]
    fn works_are_read_for_the_task_origin_and_the_first_request_only() {
        let start = json!({"prompt": "héllo", "origin": {"task": "t7"}});
        let first = json!({"tools": ["a", "b", "c"], "messages": 2});
        let second = json!({"tools": [], "messages": 9});
        let entries = vec![
            (3u64, 5u64, "work_start", &start),
            (3u64, 5u64, "model_request", &first),
            (3u64, 5u64, "model_request", &second),
        ];
        let works = works_from(&entries);
        assert_eq!(works.len(), 1);
        assert_eq!(works[0].task.as_deref(), Some("t7"));
        assert_eq!(
            (
                works[0].prompt_chars,
                works[0].tools,
                works[0].messages,
                works[0].requests
            ),
            (5, 3, 2, 2)
        );
    }

    #[test]
    fn the_frozen_split_is_deterministic_and_about_a_tenth() {
        let ids: Vec<String> = (0..10_000).map(|i| format!("t{i}")).collect();
        let first: Vec<bool> = ids
            .iter()
            .map(|id| is_frozen(Some(id), &Value::Null))
            .collect();
        let second: Vec<bool> = ids
            .iter()
            .map(|id| is_frozen(Some(id), &Value::Null))
            .collect();
        assert_eq!(first, second);
        let frozen = first.iter().filter(|f| **f).count();
        assert!((800..=1_200).contains(&frozen), "frozen {frozen} of 10000");
        let row = json!({"ts": 1, "label": "cheap"});
        assert_eq!(is_frozen(None, &row), is_frozen(None, &row));
    }

    #[test]
    fn empty_single_class_and_skew_are_flagged() {
        assert_eq!(taint_flags(&[], false), vec!["empty".to_string()]);

        let a = item("a", 1, "cheap");
        let b = item("b", 2, "cheap");
        let flags = taint_flags(&[&a, &b], false);
        assert!(flags.contains(&"single_class".to_string()));
        assert!(flags.contains(&"too_few_rows".to_string()));

        let items: Vec<Item> = (0..100)
            .map(|i| item(&format!("x{i}"), i, if i < 96 { "cheap" } else { "mid" }))
            .collect();
        let refs: Vec<&Item> = items.iter().collect();
        let flags = taint_flags(&refs, false);
        assert!(flags.contains(&"label_skew".to_string()));
        assert!(!flags.contains(&"single_class".to_string()));
        assert!(!blocking("label_skew"));
        assert!(!blocking("too_few_rows"));
    }

    #[test]
    fn upload_rejects_a_bad_line_and_takes_a_good_one() {
        let good = r#"{"ts":1700000000000,"prompt_chars":10,"prompt_tokens":3,"tool_count":1,"history_turns":0,"attachments":0,"prior_failures":0,"task_kind":"coder","domain":"x","tenant_tier":"worker","requested_model":"m","label":"mid"}"#;
        let (accepted, unknown) = parse_upload_line(good).expect("the good line");
        assert_eq!(accepted.label, "mid");
        assert!(!unknown);
        assert!(parse_upload_line(&good.replace(r#""label":"mid""#, r#""label":"huge""#)).is_err());
        assert!(
            parse_upload_line(&good.replace(r#""prompt_chars":10"#, r#""prompt_chars":"ten""#))
                .is_err()
        );
        assert!(parse_upload_line(&good.replace(r#""ts":1700000000000,"#, "")).is_err());
        assert!(parse_upload_line("[1,2]").is_err());
        let by_index = good.replace(r#""label":"mid""#, r#""label":2"#);
        assert_eq!(
            parse_upload_line(&by_index).expect("index label").0.label,
            "frontier"
        );
        let extra = good.replace('}', r#","extra":1}"#);
        assert!(parse_upload_line(&extra).is_ok_and(|(_, unknown)| unknown));
    }

    #[test]
    fn the_id_is_stable_for_the_same_bytes() {
        let a = dataset_id(b"ts,label\n1,cheap\n");
        assert_eq!(a, dataset_id(b"ts,label\n1,cheap\n"));
        assert_ne!(a, dataset_id(b"other"));
        assert!(a.starts_with("ds-") && a.len() == 3 + 16);
        let items = vec![item("a", 1, "cheap"), item("b", 2, "mid")];
        let x = assemble("n", "tasks", 5, items.clone(), false, json!({}));
        let y = assemble("n", "tasks", 9, items, false, json!({}));
        assert_eq!(x.dataset.id, y.dataset.id);
        assert_eq!(x.training, y.training);
        assert_eq!(x.dataset.file, format!("datasets/{}.jsonl", x.dataset.id));
    }

    #[test]
    fn rungs_follow_the_price_cut_points() {
        assert_eq!(rung_for_price(Some(0.5)), Some("cheap"));
        assert_eq!(rung_for_price(Some(2.0)), Some("mid"));
        assert_eq!(rung_for_price(Some(15.0)), Some("frontier"));
        assert_eq!(rung_for_price(None), None);
        assert_eq!(rung_for_price(Some(f64::NAN)), None);
    }
}
