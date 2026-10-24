//! Converts a GitHub Actions workflow file into a plaza workflow (`.plaza/workflows/<name>.toml`), with a report of what
//! converted and what did not.
//!
//! Converted: the events `push` (as `push`), `pull_request` (as `push`, with a note), `workflow_dispatch` (as `manual`),
//! `schedule` (its first cron plaza reads) and `release` published or created (as `release.candidate`); the jobs, and
//! their `needs` on jobs that run; `runs-on` (a plain label is the `any` pool, a pool key such as `arm64-hvf` is itself,
//! a `${{ matrix.KEY }}` is a template); `strategy.matrix` lists of scalars; `run` steps with `env`, `timeout-minutes`
//! and `working-directory`; `actions/checkout` (the fork already has the repo), `actions/setup-*` (a note: plaza takes the
//! toolchain from the project's stack) and `actions/upload-artifact` (a build step).
//!
//! Not converted, and listed in `Report::unsupported` with its source text: third-party and reusable-workflow `uses:`,
//! `actions/cache`, `if:`, `services`, `container`, `concurrency`, `permissions`, matrix `include` and `exclude`, branch
//! and path filters, and every expression except `${{ matrix.KEY }}` (in a run script or an env value) and
//! `${{ secrets.NAME }}` (in an env value). An untranslated expression in a run script is replaced by a shell expansion
//! that fails, so the step fails until someone edits it.
//!
//! Decisions: a `needs` on a job the file does not have, or on one with no step plaza runs, is listed and dropped, so
//! the job runs without waiting for it (GitHub rejects such a file, so the report says so). A job with no step plaza
//! runs is dropped and listed. A cycle in `needs` is an error, as `Workflow::check` makes it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;
use std::path::Path;

use serde::Serialize;
use serde_yaml_ng::{Mapping, Value};

use super::model::{Job, Known, Scalar, StepKind, Workflow, valid_name};

/// What a conversion mapped, noted and did not convert.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    /// One line per thing that became a plaza thing (`jobs.build.steps[1]: run -> run step`).
    pub mapped: Vec<String>,
    /// Things that were rewritten, or that the owner should read, without being unsupported.
    pub notes: Vec<String>,
    /// Things plaza does not run; each says where it is and what happened to it.
    pub unsupported: Vec<Unsupported>,
}

/// One thing of the source that plaza does not run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Unsupported {
    /// Where it is in the file (`jobs.test.steps[2]`).
    pub at: String,
    /// Its source text (`uses: some-org/action@v2`).
    pub what: String,
    /// Why plaza does not run it, and what the conversion did with it.
    pub reason: String,
}

/// A converted workflow file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Imported {
    /// The workflow's name: its file's stem.
    pub name: String,
    /// The file's repo path.
    pub file: String,
    /// The plaza workflow, which passes `Workflow::check`.
    pub workflow: Workflow,
    /// What converted, what was noted and what did not.
    pub report: Report,
}

impl Report {
    fn map(&mut self, at: impl Display, what: impl Display) {
        self.mapped.push(format!("{at}: {what}"));
    }

    fn note(&mut self, text: impl Display) {
        self.notes.push(text.to_string());
    }

    fn unsupported(&mut self, at: impl Display, what: impl Display, reason: impl Display) {
        self.unsupported.push(Unsupported {
            at: at.to_string(),
            what: what.to_string(),
            reason: reason.to_string(),
        });
    }
}

/// Converts one GitHub Actions workflow file. `file` is its repo path (e.g. `.github/workflows/ci.yml`); the workflow's
/// name is the file stem. Err only when the YAML is not a workflow at all (not a mapping, or no `jobs`), or when the
/// converted workflow does not pass `Workflow::check` (a cycle in `needs`).
pub fn convert(file: &str, yaml: &str) -> Result<Imported, String> {
    let doc: Value =
        serde_yaml_ng::from_str(yaml).map_err(|e| format!("{file} is not YAML: {e}"))?;
    let Value::Mapping(top) = &doc else {
        return Err(format!(
            "{file} is not a workflow: its YAML is not a mapping"
        ));
    };
    let Some(Value::Mapping(jobs)) = find(top, "jobs") else {
        return Err(format!("{file} is not a workflow: it has no jobs"));
    };
    let mut report = Report::default();
    let name = workflow_name(file, &mut report);
    let description = match find(top, "name") {
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    };
    let (triggers, cron) = read_triggers(top, &mut report);
    for (key, value) in top.iter() {
        let key = key_text(key);
        let reason = match key.as_str() {
            "name" | "on" | "jobs" => continue,
            "env" | "permissions" | "concurrency" | "defaults" => {
                "plaza has no workflow-level equivalent, so it is not translated"
            }
            _ => "plaza does not read this key",
        };
        report.unsupported(&key, format!("{key}: {}", show(value)), reason);
    }

    let mut ids = BTreeSet::new();
    let drafts: Vec<Draft> = jobs
        .iter()
        .map(|(key, value)| convert_job(&key_text(key), value, &mut ids, &mut report))
        .collect();
    let sources: BTreeSet<String> = drafts.iter().map(|d| d.source.clone()).collect();
    let kept: BTreeMap<String, String> = drafts
        .iter()
        .filter(|d| !d.job.steps.is_empty())
        .map(|d| (d.source.clone(), d.job.id.clone()))
        .collect();
    let mut out = Vec::new();
    for draft in drafts {
        let Draft {
            source,
            mut job,
            needs,
        } = draft;
        let at = format!("jobs.{source}");
        if job.steps.is_empty() {
            report.unsupported(
                &at,
                "steps",
                "no step of this job is one plaza runs, so the job is dropped",
            );
            continue;
        }
        for need in needs {
            let at_needs = format!("{at}.needs");
            if need == source {
                report.unsupported(
                    &at_needs,
                    &need,
                    "a job cannot need itself; the need is dropped",
                );
            } else if let Some(id) = kept.get(&need) {
                add(&mut job.needs, id);
            } else {
                let reason = if sources.contains(&need) {
                    "that job has no step plaza runs; this job does not wait for it"
                } else {
                    "the file has no such job; this job does not wait for it"
                };
                report.unsupported(&at_needs, format!("needs: {need}"), reason);
            }
        }
        out.push(job);
    }

    let workflow = Workflow {
        name: name.clone(),
        description,
        triggers,
        cron,
        jobs: out,
    };
    workflow
        .check(&Known::default())
        .map_err(|e| format!("{file}: the converted workflow does not check: {e}"))?;
    Ok(Imported {
        name,
        file: file.to_string(),
        workflow,
        report,
    })
}

/// The workflow's name: the file's stem, sanitized when it is not a plaza name.
fn workflow_name(file: &str, report: &mut Report) -> String {
    let stem = Path::new(file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if valid_name(stem) {
        return stem.to_string();
    }
    let sanitized = sanitize(stem);
    let name = if sanitized.is_empty() {
        "workflow".to_string()
    } else {
        sanitized
    };
    report.note(format!(
        "{file}: the workflow is named {name}; its file stem is not a plaza name"
    ));
    name
}

/// The triggers and the cron of the `on:` key.
fn read_triggers(top: &Mapping, report: &mut Report) -> (Vec<String>, String) {
    let mut triggers = Vec::new();
    let mut cron = String::new();
    let events: Vec<(String, Option<&Value>)> = match find(top, "on") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(event)) => vec![(event.clone(), None)],
        Some(Value::Sequence(items)) => items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| match item {
                Value::String(event) => Some((event.clone(), None)),
                other => {
                    report.unsupported(format!("on[{i}]"), show(other), "an event is a name");
                    None
                }
            })
            .collect(),
        Some(Value::Mapping(events)) => events
            .iter()
            .map(|(key, config)| (key_text(key), Some(config)))
            .collect(),
        Some(other) => {
            report.unsupported(
                "on",
                show(other),
                "the events are a name, a list or a mapping",
            );
            Vec::new()
        }
    };
    for (event, config) in events {
        match event.as_str() {
            "push" => {
                add(&mut triggers, "push");
                report.map("on.push", "trigger push");
                filters(report, "push", config);
            }
            "pull_request" => {
                add(&mut triggers, "push");
                report.map("on.pull_request", "trigger push");
                report.note(
                    "on.pull_request runs as a push: plaza has no pull request trigger, so the workflow runs on the pushed commits",
                );
                filters(report, "pull_request", config);
            }
            "workflow_dispatch" => {
                add(&mut triggers, "manual");
                report.map("on.workflow_dispatch", "manual");
                filters(report, "workflow_dispatch", config);
            }
            "schedule" => schedule(config, &mut triggers, &mut cron, report),
            "release" => release(config, &mut triggers, report),
            other => report.unsupported(
                format!("on.{other}"),
                other,
                "plaza has no trigger for this event",
            ),
        }
    }
    if !triggers.iter().any(|t| t == "manual") {
        add(&mut triggers, "manual");
        report.note(
            "manual: plaza runs a workflow by hand, so the workflow is listed as manual although the file names no manual trigger",
        );
    }
    (triggers, cron)
}

/// The filters of a trigger: plaza's triggers take none, so each one is listed and dropped.
fn filters(report: &mut Report, event: &str, config: Option<&Value>) {
    let Some(Value::Mapping(config)) = config else {
        return;
    };
    for (key, value) in config.iter() {
        let key = key_text(key);
        let reason = format!("plaza's {event} trigger has no {key}, so it is dropped");
        report.unsupported(
            format!("on.{event}.{key}"),
            format!("{key}: {}", show(value)),
            reason,
        );
    }
}

/// The `schedule` trigger: the first cron plaza reads is the workflow's cron; the others are listed.
fn schedule(
    config: Option<&Value>,
    triggers: &mut Vec<String>,
    cron: &mut String,
    report: &mut Report,
) {
    let Some(Value::Sequence(items)) = config else {
        let what = config.map(show).unwrap_or_default();
        report.unsupported("on.schedule", what, "a schedule is a list of cron entries");
        return;
    };
    for (i, item) in items.iter().enumerate() {
        let at = format!("on.schedule[{i}]");
        let expr = match item {
            Value::Mapping(entry) => match find(entry, "cron") {
                Some(Value::String(expr)) => Some(expr.clone()),
                _ => None,
            },
            _ => None,
        };
        let Some(expr) = expr else {
            report.unsupported(&at, show(item), "a schedule entry is a mapping with a cron");
            continue;
        };
        if let Err(e) = crate::triggers::cron::parse(&expr) {
            report.unsupported(
                &at,
                format!("cron: {expr}"),
                format!("plaza cannot read this cron: {e}"),
            );
            continue;
        }
        if !cron.is_empty() {
            report.unsupported(
                &at,
                format!("cron: {expr}"),
                "plaza runs one cron per workflow; the first is kept",
            );
            continue;
        }
        cron.push_str(&expr);
        add(triggers, "schedule");
        report.map(&at, format!("cron {expr} -> schedule trigger"));
    }
}

/// The `release` trigger: published (the default) and created releases are `release.candidate`.
fn release(config: Option<&Value>, triggers: &mut Vec<String>, report: &mut Report) {
    let mut types: Vec<String> = vec!["published".to_string()];
    match config {
        None | Some(Value::Null) => {}
        Some(Value::Mapping(config)) => {
            for (key, value) in config.iter() {
                let key = key_text(key);
                if key != "types" {
                    report.unsupported(
                        format!("on.release.{key}"),
                        format!("{key}: {}", show(value)),
                        "plaza's release trigger has no such key",
                    );
                }
            }
            if let Some(Value::Sequence(items)) = find(config, "types") {
                types = items.iter().filter_map(scalar).collect();
            }
        }
        Some(other) => {
            report.unsupported("on.release", show(other), "a release trigger is a mapping");
            return;
        }
    }
    if !types.iter().any(|t| is_release_type(t)) {
        report.unsupported(
            "on.release",
            format!("types: {}", types.join(", ")),
            "plaza's release.candidate runs on published or created releases",
        );
        return;
    }
    for t in types.iter().filter(|t| !is_release_type(t)) {
        report.unsupported(
            "on.release.types",
            t,
            "plaza's release.candidate runs on published or created releases only",
        );
    }
    add(triggers, "release.candidate");
    report.map("on.release", "release.candidate");
}

fn is_release_type(event_type: &str) -> bool {
    matches!(event_type, "published" | "created")
}

/// A job as the file converts it, before its `needs` are resolved. Its needs are the file's job ids.
struct Draft {
    source: String,
    job: Job,
    needs: Vec<String>,
}

/// What a job's steps read from the job.
struct JobCtx<'a> {
    at: &'a str,
    env: BTreeMap<String, String>,
    matrix: &'a BTreeSet<String>,
    timeout: Option<u64>,
}

/// Converts one job. A job with no step plaza runs comes back with no steps, and the caller drops it.
fn convert_job(
    source: &str,
    value: &Value,
    ids: &mut BTreeSet<String>,
    report: &mut Report,
) -> Draft {
    let at = format!("jobs.{source}");
    let id = unique_id(source, ids, report);
    let mut job = Job {
        id,
        needs: Vec::new(),
        runs_on: "any".to_string(),
        matrix: BTreeMap::new(),
        steps: Vec::new(),
    };
    let Value::Mapping(map) = value else {
        report.unsupported(&at, show(value), "a job is a mapping of keys");
        return Draft {
            source: source.to_string(),
            job,
            needs: Vec::new(),
        };
    };
    if let Some(strategy_value) = find(map, "strategy") {
        job.matrix = strategy(&at, strategy_value, report);
    }
    if !job.matrix.is_empty() {
        report.map(
            format!("{at}.strategy.matrix"),
            format!("{} key(s) -> job matrix", job.matrix.len()),
        );
    }
    let matrix: BTreeSet<String> = job.matrix.keys().cloned().collect();
    job.runs_on = runs_on(&at, find(map, "runs-on"), &matrix, report);
    let env = env_map(
        &at,
        find(map, "env"),
        &matrix,
        "merged into each run step",
        report,
    );
    let timeout = find(map, "timeout-minutes")
        .and_then(|v| minutes(&format!("{at}.timeout-minutes"), v, report));
    if timeout.is_some() {
        report.note(format!(
            "{at}: timeout-minutes applies to each run step that sets none"
        ));
    }
    let mut needs = Vec::new();
    for (key, value) in map.iter() {
        let key = key_text(key);
        match key.as_str() {
            "needs" => needs = needs_list(&at, value, report),
            "name" => report.note(format!("{at}: name {}", show(value))),
            "steps" | "strategy" | "runs-on" | "env" | "timeout-minutes" => {}
            "if" => drop_key(
                report,
                &at,
                &key,
                value,
                "the job runs always; the condition is not translated",
            ),
            "continue-on-error" => drop_key(
                report,
                &at,
                &key,
                value,
                "plaza has no continue-on-error; a failing job fails the run",
            ),
            "services" => drop_key(
                report,
                &at,
                &key,
                value,
                "plaza's fork has no service containers",
            ),
            "container" => drop_key(
                report,
                &at,
                &key,
                value,
                "plaza runs jobs in the project's fork, not in a container",
            ),
            "defaults" | "working-directory" => {
                drop_key(report, &at, &key, value, "plaza has no job-level defaults")
            }
            "concurrency" | "permissions" => drop_key(
                report,
                &at,
                &key,
                value,
                "plaza has no equivalent, so it is not translated",
            ),
            "outputs" => drop_key(
                report,
                &at,
                &key,
                value,
                "plaza passes no outputs between jobs",
            ),
            "environment" => drop_key(
                report,
                &at,
                &key,
                value,
                "plaza runs environments through a deploy step",
            ),
            "uses" | "with" | "secrets" => drop_key(
                report,
                &at,
                &key,
                value,
                "a reusable workflow is not run; plaza has no reusable workflows",
            ),
            _ => drop_key(report, &at, &key, value, "plaza does not read this key"),
        }
    }
    let ctx = JobCtx {
        at: &at,
        env,
        matrix: &matrix,
        timeout,
    };
    match find(map, "steps") {
        Some(Value::Sequence(steps)) => {
            for (index, raw) in steps.iter().enumerate() {
                convert_step(&ctx, index, raw, &mut job.steps, report);
            }
        }
        Some(Value::Null) | None => {}
        Some(other) => drop_key(report, &at, "steps", other, "steps is a list"),
    }
    Draft {
        source: source.to_string(),
        job,
        needs,
    }
}

/// A plaza job id for a source job id: the id with the characters plaza does not take as dashes, and a number added
/// when it repeats an id already taken.
fn unique_id(source: &str, ids: &mut BTreeSet<String>, report: &mut Report) -> String {
    let sanitized = sanitize(source);
    let base = if sanitized.is_empty() {
        "job".to_string()
    } else {
        sanitized
    };
    let mut id = base.clone();
    let mut n = 2;
    while ids.contains(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    if id != source {
        report.note(format!(
            "jobs.{source}: the job is named {id} (plaza job ids are letters, digits, dashes and underscores)"
        ));
    }
    ids.insert(id.clone());
    id
}

/// The `strategy` block: the matrix is kept, fail-fast is a note, and max-parallel and other keys are listed.
fn strategy(at: &str, value: &Value, report: &mut Report) -> BTreeMap<String, Vec<Scalar>> {
    let at = format!("{at}.strategy");
    let Value::Mapping(map) = value else {
        report.unsupported(&at, show(value), "strategy is a mapping");
        return BTreeMap::new();
    };
    let mut matrix = BTreeMap::new();
    for (key, value) in map.iter() {
        let key = key_text(key);
        match key.as_str() {
            "matrix" => matrix = matrix_of(&at, value, report),
            "fail-fast" => report.note(format!(
                "{at}.fail-fast: plaza runs all matrix jobs in parallel, so one failure does not stop the others"
            )),
            "max-parallel" => drop_key(
                report,
                &at,
                &key,
                value,
                "plaza runs matrix jobs all in parallel; there is no limit",
            ),
            _ => drop_key(report, &at, &key, value, "plaza does not read this key"),
        }
    }
    matrix
}

/// The matrix: each key's list of values. `include` and `exclude` are listed and dropped.
fn matrix_of(at: &str, value: &Value, report: &mut Report) -> BTreeMap<String, Vec<Scalar>> {
    let mut matrix = BTreeMap::new();
    let Value::Mapping(map) = value else {
        report.unsupported(
            at,
            show(value),
            "a matrix is a mapping of keys to lists; an expression is not translated",
        );
        return matrix;
    };
    for (key, value) in map.iter() {
        let key = key_text(key);
        let at = format!("{at}.matrix.{key}");
        if key == "include" || key == "exclude" {
            report.unsupported(
                &at,
                format!("{key}: {}", show(value)),
                "matrix include and exclude are not translated; their entries are dropped",
            );
            continue;
        }
        if !valid_name(&key) {
            report.unsupported(
                &at,
                show(value),
                "a matrix key is letters, digits, dashes and underscores",
            );
            continue;
        }
        let Value::Sequence(items) = value else {
            report.unsupported(
                &at,
                show(value),
                "a matrix key takes a list of values; an expression is not translated",
            );
            continue;
        };
        let values = matrix_values(&at, items, report);
        if values.is_empty() {
            report.unsupported(&at, show(value), "the key has no values, so it is dropped");
        } else {
            matrix.insert(key, values);
        }
    }
    matrix
}

/// The values of one matrix key: texts, integers, decimals (kept as text, with a note) and booleans.
fn matrix_values(at: &str, items: &[Value], report: &mut Report) -> Vec<Scalar> {
    let mut values = Vec::new();
    for item in items {
        match item {
            Value::String(text) if text.contains("${{") => report.unsupported(
                at,
                text,
                "an expression in a matrix value is not translated",
            ),
            Value::String(text) => values.push(Scalar::Text(text.clone())),
            Value::Bool(flag) => values.push(Scalar::Bool(*flag)),
            Value::Number(number) => match number.as_i64() {
                Some(int) => values.push(Scalar::Int(int)),
                None => {
                    report.note(format!(
                        "{at}: the value {number} is a decimal; quote it in the matrix to keep its text"
                    ));
                    values.push(Scalar::Text(number.to_string()));
                }
            },
            other => report.unsupported(
                at,
                show(other),
                "a matrix value is a text, a number or a boolean",
            ),
        }
    }
    values
}

/// The pool a job runs on. GitHub's own labels are the `any` pool (with a note), a plaza pool key is itself, and a
/// matrix value is a template. Anything else is listed, and the job runs on `any`.
fn runs_on(
    at: &str,
    value: Option<&Value>,
    matrix: &BTreeSet<String>,
    report: &mut Report,
) -> String {
    let at = format!("{at}.runs-on");
    let labels: Vec<String> = match value {
        None | Some(Value::Null) => return "any".to_string(),
        Some(Value::String(label)) => vec![label.clone()],
        Some(Value::Sequence(items)) => {
            let mut labels = Vec::new();
            for item in items {
                match item {
                    Value::String(label) => labels.push(label.clone()),
                    other => report.unsupported(&at, show(other), "a label is a text"),
                }
            }
            labels
        }
        Some(other) => {
            report.unsupported(&at, show(other), "runs-on is a label or a list of labels");
            return "any".to_string();
        }
    };
    let mut pool: Option<String> = None;
    for label in labels {
        if label.contains("${{") {
            if !all_matrix(&label, matrix) {
                report.unsupported(
                    &at,
                    &label,
                    "the expression is not a matrix value of the job, so the job runs on any",
                );
                continue;
            }
        } else if !is_pool_key(&label) {
            report.note(format!(
                "{at}: label {label} is not a plaza pool; the job runs on the any pool"
            ));
            continue;
        }
        if pool.is_some() {
            report.unsupported(
                &at,
                &label,
                "a job has one pool; this label is not translated",
            );
        } else {
            pool = Some(label);
        }
    }
    pool.unwrap_or_else(|| "any".to_string())
}

/// Whether every expression of a template is a matrix key of the job.
fn all_matrix(template: &str, matrix: &BTreeSet<String>) -> bool {
    spans(template).iter().all(|span| {
        span.body
            .as_deref()
            .and_then(|body| body.strip_prefix("matrix."))
            .is_some_and(|key| matrix.contains(key))
    })
}

/// A plaza pool key, `arch-hypervisor` such as `arm64-hvf`: two lowercase parts with one dash. GitHub's own labels
/// (`ubuntu-latest`, `macos-14`, `windows-2022`, `self-hosted`) are not pool keys.
fn is_pool_key(label: &str) -> bool {
    let Some((arch, hypervisor)) = label.split_once('-') else {
        return false;
    };
    let part = |p: &str| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    };
    part(arch) && part(hypervisor) && !matches!(arch, "ubuntu" | "macos" | "windows" | "self")
}

/// The `needs` of a job: a job id or a list of them.
fn needs_list(at: &str, value: &Value, report: &mut Report) -> Vec<String> {
    match value {
        Value::String(job) => vec![job.clone()],
        Value::Sequence(items) => {
            let mut needs = Vec::new();
            for item in items {
                match item {
                    Value::String(job) => needs.push(job.clone()),
                    other => report.unsupported(
                        format!("{at}.needs"),
                        show(other),
                        "a needs entry is a job id",
                    ),
                }
            }
            needs
        }
        other => {
            report.unsupported(
                format!("{at}.needs"),
                show(other),
                "needs is a job id or a list of them",
            );
            Vec::new()
        }
    }
}

/// The seconds of a `timeout-minutes` value: a whole number of minutes, 1 or more. Other values are listed.
fn minutes(at: &str, value: &Value, report: &mut Report) -> Option<u64> {
    let seconds = value
        .as_i64()
        .and_then(|m| u64::try_from(m).ok())
        .filter(|m| *m >= 1)
        .and_then(|m| m.checked_mul(60));
    if seconds.is_none() {
        report.unsupported(
            at,
            show(value),
            "timeout-minutes is a whole number of minutes, 1 or more; the default timeout applies",
        );
    }
    seconds
}

/// Converts one step of a job: a run step, or an action step (which may be no step at all).
fn convert_step(
    ctx: &JobCtx,
    index: usize,
    raw: &Value,
    steps: &mut Vec<StepKind>,
    report: &mut Report,
) {
    let at = format!("{}.steps[{index}]", ctx.at);
    let Value::Mapping(map) = raw else {
        report.unsupported(&at, show(raw), "a step is a mapping of keys");
        return;
    };
    for (key, value) in map.iter() {
        let key = key_text(key);
        match key.as_str() {
            "name" => report.note(format!("{at}: name {}", show(value))),
            "id" => report.note(format!("{at}: id {} is kept as a note only", show(value))),
            "if" => drop_key(
                report,
                &at,
                &key,
                value,
                "the step runs always; the condition is not translated",
            ),
            "continue-on-error" => drop_key(
                report,
                &at,
                &key,
                value,
                "plaza has no continue-on-error; a failing step fails the job",
            ),
            "run" | "uses" | "shell" | "working-directory" | "env" | "timeout-minutes" | "with" => {
            }
            _ => drop_key(report, &at, &key, value, "plaza does not read this key"),
        }
    }
    match (find(map, "run"), find(map, "uses")) {
        (None, None) => report.unsupported(
            &at,
            show(raw),
            "the step has neither run nor uses, so it is dropped",
        ),
        (Some(run), None) => run_step(ctx, &at, map, run, steps, report),
        (None, Some(uses)) => uses_step(&at, map, uses, steps, report),
        (Some(run), Some(uses)) => {
            report.unsupported(
                &at,
                format!("uses: {}", show(uses)),
                "a step is a run command or an action; the action is not run, and only the run command is kept",
            );
            run_step(ctx, &at, map, run, steps, report);
        }
    }
}

/// A run step: its command with the expressions checked, its working directory, its env and its timeout.
fn run_step(
    ctx: &JobCtx,
    at: &str,
    map: &Mapping,
    script: &Value,
    steps: &mut Vec<StepKind>,
    report: &mut Report,
) {
    let Value::String(script) = script else {
        report.unsupported(
            at,
            format!("run: {}", show(script)),
            "run is the command text",
        );
        return;
    };
    if script.trim().is_empty() {
        report.unsupported(
            at,
            "run: (empty)",
            "the run command is empty, so the step is dropped",
        );
        return;
    }
    if let Some(shell) = find(map, "shell")
        && !matches!(shell, Value::String(s) if s == "bash" || s == "sh")
    {
        report.unsupported(
            format!("{at}.shell"),
            format!("shell: {}", show(shell)),
            "plaza runs the step in its own shell; the shell is not translated",
        );
    }
    if let Some(with) = find(map, "with") {
        drop_key(
            report,
            at,
            "with",
            with,
            "only actions take with; a run step has no inputs",
        );
    }
    let mut command = translate(at, script, ctx.matrix, report);
    let mut mapped = "run -> run step".to_string();
    if let Some(dir) = find(map, "working-directory") {
        match dir {
            Value::String(dir) => {
                let dir = translate(&format!("{at}.working-directory"), dir, ctx.matrix, report);
                command = in_dir(&dir, &command);
                mapped = format!("run in {dir} -> run step with cd");
            }
            other => report.unsupported(
                format!("{at}.working-directory"),
                show(other),
                "working-directory is a path",
            ),
        }
    }
    let mut env = ctx.env.clone();
    env.extend(env_map(
        at,
        find(map, "env"),
        ctx.matrix,
        "run step env",
        report,
    ));
    let timeout = match find(map, "timeout-minutes") {
        Some(value) => minutes(&format!("{at}.timeout-minutes"), value, report).or(ctx.timeout),
        None => ctx.timeout,
    };
    report.map(at, mapped);
    steps.push(StepKind::Run {
        run: command,
        env,
        timeout,
    });
}

/// An action step. The checkout and the toolchains are implicit, an artifact upload is the build step, and the other
/// actions are listed.
fn uses_step(
    at: &str,
    map: &Mapping,
    uses: &Value,
    steps: &mut Vec<StepKind>,
    report: &mut Report,
) {
    let Value::String(uses) = uses else {
        report.unsupported(
            at,
            format!("uses: {}", show(uses)),
            "uses is an action reference",
        );
        return;
    };
    for key in ["env", "shell", "working-directory", "timeout-minutes"] {
        if let Some(value) = find(map, key) {
            drop_key(
                report,
                at,
                key,
                value,
                "an action step runs no command, so the key has no effect",
            );
        }
    }
    let action = uses.split('@').next().unwrap_or(uses.as_str());
    let inputs = with_inputs(map);
    match action {
        "actions/checkout" => {
            report.map(at, format!("{action} -> implicit checkout"));
            if !inputs.is_empty() {
                report.unsupported(
                    format!("{at}.with"),
                    format!("with: {}", pairs(&inputs)),
                    "checkout options are not translated; the fork already has the project at the run's commit",
                );
            }
        }
        "actions/setup-node"
        | "actions/setup-python"
        | "actions/setup-go"
        | "actions/setup-java"
        | "actions/setup-dotnet" => {
            let shown = if inputs.is_empty() {
                String::new()
            } else {
                format!(" ({})", pairs(&inputs))
            };
            report.note(format!(
                "{at}: {uses}{shown} is not a step: plaza takes the toolchain from the project's stack, not from the workflow"
            ));
        }
        "actions/cache" => report.unsupported(
            at,
            format!("uses: {uses}"),
            "plaza's fork caches the build itself; the cache step is dropped",
        ),
        "actions/upload-artifact" => {
            let name = input_of(&inputs, "name").unwrap_or_else(|| "artifact".to_string());
            let path = input_of(&inputs, "path").unwrap_or_default();
            report.note(format!(
                "{at}: uploads artifact {name} (path {path}); plaza's build step produces the project's build output"
            ));
            if steps.iter().any(|step| matches!(step, StepKind::Build {})) {
                report.map(at, "upload-artifact -> covered by the job's build step");
            } else {
                steps.push(StepKind::Build {});
                report.map(at, "upload-artifact -> build step");
            }
        }
        "actions/download-artifact" => report.unsupported(
            at,
            format!("uses: {uses}"),
            "plaza passes no artifacts between jobs; the step is dropped",
        ),
        _ => report.unsupported(
            at,
            format!("uses: {uses}"),
            "plaza has no action runner; the step needs a run: command instead",
        ),
    }
}

/// The text of a run script with its expressions checked. `${{ matrix.KEY }}` of the job stays. Any other expression is
/// listed and replaced by a shell expansion that fails, so the step fails until it is edited.
fn translate(at: &str, text: &str, matrix: &BTreeSet<String>, report: &mut Report) -> String {
    let mut out = String::new();
    let mut last = 0;
    for span in spans(text) {
        out.push_str(&text[last..span.start]);
        last = span.end;
        let source = &text[span.start..span.end];
        match run_problem(span.body.as_deref(), matrix) {
            None => out.push_str(source),
            Some(problem) => {
                report.unsupported(
                    at,
                    source,
                    format!("{problem}; the expression is not translated and the step will fail to run until it is edited"),
                );
                out.push_str(&failing(span.body.as_deref().unwrap_or("unclosed")));
            }
        }
    }
    out.push_str(&text[last..]);
    out
}

/// Why an expression of a run script is not plaza's, or `None` when it is `${{ matrix.KEY }}` of the job.
fn run_problem(body: Option<&str>, matrix: &BTreeSet<String>) -> Option<String> {
    let Some(body) = body else {
        return Some("the expression does not close".to_string());
    };
    if let Some(key) = body.strip_prefix("matrix.") {
        return (!matrix.contains(key)).then(|| format!("{body} names no matrix key of the job"));
    }
    if body.starts_with("secrets.") {
        return Some("secrets are read only in env values".to_string());
    }
    Some("it is not translated".to_string())
}

/// A shell expansion that fails with a note, standing in for an expression plaza does not translate.
fn failing(expression: &str) -> String {
    let note: String = expression
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("${{GH_EXPR_NOT_TRANSLATED:?{note} is not translated, edit this step}}")
}

/// The env of a block (`prefix` is the block's path): each value checked. A value with an expression plaza does not
/// read is dropped and listed by name; the others are kept, and `kept` says so in the report.
fn env_map(
    prefix: &str,
    value: Option<&Value>,
    matrix: &BTreeSet<String>,
    kept: &str,
    report: &mut Report,
) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    let Some(value) = value else {
        return env;
    };
    let Value::Mapping(map) = value else {
        report.unsupported(
            format!("{prefix}.env"),
            show(value),
            "env is a mapping of names to values",
        );
        return env;
    };
    for (key, value) in map.iter() {
        let name = key_text(key);
        let at = format!("{prefix}.env.{name}");
        if !valid_name(&name) {
            report.unsupported(
                &at,
                format!("{name}: {}", show(value)),
                "an env name is letters, digits and underscores",
            );
            continue;
        }
        let text = match scalar(value) {
            Some(text) => text,
            None => {
                report.unsupported(
                    &at,
                    format!("{name}: {}", show(value)),
                    "an env value is a text, a number or a boolean",
                );
                continue;
            }
        };
        match env_problem(&text, matrix) {
            None => {
                report.map(&at, kept);
                env.insert(name, text);
            }
            Some(problem) => report.unsupported(&at, format!("{name}: {text}"), problem),
        }
    }
    env
}

/// Why an env value is not plaza's: an expression plaza does not read. `None` when every expression is a secret by its
/// vault name or a matrix key of the job.
fn env_problem(text: &str, matrix: &BTreeSet<String>) -> Option<String> {
    for span in spans(text) {
        let Some(body) = span.body.as_deref() else {
            return Some("the expression does not close".to_string());
        };
        if let Some(name) = body.strip_prefix("secrets.") {
            if valid_name(name) {
                continue;
            }
            return Some(format!(
                "{body}: a secret is named by letters, digits and underscores"
            ));
        }
        if let Some(key) = body.strip_prefix("matrix.") {
            if matrix.contains(key) {
                continue;
            }
            return Some(format!("${{{{ {body} }}}} names no matrix key of the job"));
        }
        if body.starts_with("env.") {
            return Some(format!("${{{{ {body} }}}} has no equivalent in plaza"));
        }
        return Some(format!("${{{{ {body} }}}} is not translated"));
    }
    None
}

/// One `${{ ... }}` in a text: its byte range (the braces included) and its trimmed body, `None` when it does not close.
struct Span {
    start: usize,
    end: usize,
    body: Option<String>,
}

fn spans(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(found) = text[pos..].find("${{") {
        let start = pos + found;
        let open = start + 3;
        match text[open..].find("}}") {
            Some(close) => {
                let end = open + close + 2;
                out.push(Span {
                    start,
                    end,
                    body: Some(text[open..open + close].trim().to_string()),
                });
                pos = end;
            }
            None => {
                out.push(Span {
                    start,
                    end: open,
                    body: None,
                });
                pos = open;
            }
        }
    }
    out
}

/// The command run in `dir`: `cd dir && command`, or the `cd` on its own line before a multi-line script.
fn in_dir(dir: &str, command: &str) -> String {
    let plain = !dir.is_empty()
        && dir
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+@:".contains(c));
    let dir = if plain {
        dir.to_string()
    } else {
        format!("'{}'", dir.replace('\'', "'\\''"))
    };
    if command.contains('\n') {
        format!("cd {dir} || exit 1\n{command}")
    } else {
        format!("cd {dir} && {command}")
    }
}

/// The `with:` inputs of a step, as (name, text) pairs.
fn with_inputs(map: &Mapping) -> Vec<(String, String)> {
    match find(map, "with") {
        Some(Value::Mapping(with)) => with
            .iter()
            .map(|(key, value)| (key_text(key), show(value)))
            .collect(),
        _ => Vec::new(),
    }
}

fn input_of(inputs: &[(String, String)], name: &str) -> Option<String> {
    inputs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

fn pairs(inputs: &[(String, String)]) -> String {
    inputs
        .iter()
        .map(|(key, value)| format!("{key}: {value}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Lists a key of a block that is not translated: `at.key`, with the key's source text.
fn drop_key(report: &mut Report, at: &str, key: &str, value: &Value, reason: &str) {
    report.unsupported(
        format!("{at}.{key}"),
        format!("{key}: {}", show(value)),
        reason,
    );
}

/// Adds `item` to `list` unless it is there already.
fn add(list: &mut Vec<String>, item: &str) {
    if !list.iter().any(|known| known == item) {
        list.push(item.to_string());
    }
}

/// The value of a mapping's key, by the key's text.
fn find<'a>(map: &'a Mapping, name: &str) -> Option<&'a Value> {
    map.iter()
        .find(|(key, _)| key_text(key) == name)
        .map(|(_, value)| value)
}

/// A mapping key's text. YAML 1.1 reads an unquoted `on` as `true`, so that key is `on`.
fn key_text(key: &Value) -> String {
    match key {
        Value::Bool(true) => "on".to_string(),
        other => scalar(other).unwrap_or_else(|| "?".to_string()),
    }
}

/// A scalar's text: a text, a number, a boolean, or the empty text for null. Other values have none.
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Number(number) => Some(number.to_string()),
        Value::Null => Some(String::new()),
        _ => None,
    }
}

/// A value's text for a report line: a scalar as it is, other values as their YAML on one line.
fn show(value: &Value) -> String {
    match scalar(value) {
        Some(text) => text,
        None => serde_yaml_ng::to_string(value)
            .unwrap_or_default()
            .trim()
            .replace('\n', "; "),
    }
}

/// The text with each character a plaza name does not take replaced by a dash.
fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CI: &str = r#"name: CI
on: [push, workflow_dispatch]
jobs:
  test:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        node: [18, 20]
        os: [linux]
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: ${{ matrix.node }}
      - run: npm ci
      - name: Test
        run: npm test
        env:
          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}
      - uses: some-org/deploy-thing@v2
  release:
    needs: test
    runs-on: ubuntu-latest
    steps:
      - run: echo hi
"#;

    fn job<'a>(workflow: &'a Workflow, id: &str) -> &'a Job {
        workflow
            .jobs
            .iter()
            .find(|j| j.id == id)
            .expect("the job is in the workflow")
    }

    #[test]
    fn converts_a_realistic_ci_file() {
        let imported = convert(".github/workflows/ci.yml", CI).expect("the fixture converts");
        let wf = &imported.workflow;
        assert_eq!(wf.name, "ci");
        assert_eq!(wf.description, "CI");
        assert!(wf.runs_on_trigger("push"));
        assert!(wf.runs_on_trigger("manual"));
        let test = job(wf, "test");
        assert_eq!(
            test.matrix.get("node"),
            Some(&vec![Scalar::Int(18), Scalar::Int(20)])
        );
        assert_eq!(
            test.matrix.get("os"),
            Some(&vec![Scalar::Text("linux".to_string())])
        );
        assert_eq!(job(wf, "release").needs, vec!["test".to_string()]);
        assert!(
            imported
                .report
                .unsupported
                .iter()
                .any(|u| u.what.contains("some-org/deploy-thing"))
        );
        assert!(
            imported
                .report
                .mapped
                .iter()
                .any(|m| m.contains("actions/checkout") && m.contains("implicit checkout"))
        );
        assert!(
            imported
                .report
                .notes
                .iter()
                .any(|n| n.contains("actions/setup-node@v4"))
        );
        assert_eq!(
            test.steps.len(),
            2,
            "checkout and setup-node are not steps, and the deploy action is listed"
        );
        let env = test
            .steps
            .iter()
            .find_map(|step| match step {
                StepKind::Run { run, env, .. } if run == "npm test" => Some(env),
                _ => None,
            })
            .expect("the npm test step");
        assert_eq!(
            env.get("NPM_TOKEN").map(String::as_str),
            Some("${{ secrets.NPM_TOKEN }}")
        );
    }

    #[test]
    fn the_converted_workflow_round_trips_through_its_toml() {
        let imported = convert("ci.yml", CI).expect("the fixture converts");
        let text = imported.workflow.to_toml();
        let parsed = Workflow::parse("ci", &text).expect("the TOML parses and checks");
        assert_eq!(parsed, imported.workflow);
    }

    #[test]
    fn a_need_of_a_missing_job_is_listed_and_not_waited_for() {
        let yaml = "on: push\njobs:\n  build:\n    needs: missing\n    steps:\n      - run: make\n";
        let imported = convert("build.yml", yaml).expect("the file converts");
        assert!(job(&imported.workflow, "build").needs.is_empty());
        assert!(
            imported
                .report
                .unsupported
                .iter()
                .any(|u| u.at == "jobs.build.needs" && u.what.contains("missing"))
        );
    }

    #[test]
    fn a_schedule_sets_the_cron_and_an_untranslated_expression_is_listed() {
        let yaml = "on:\n  schedule:\n    - cron: '0 6 * * *'\njobs:\n  check:\n    steps:\n      - run: echo ${{ github.sha }}\n";
        let imported = convert("nightly.yml", yaml).expect("the file converts");
        assert_eq!(imported.workflow.cron, "0 6 * * *");
        assert!(imported.workflow.runs_on_trigger("schedule"));
        assert!(
            imported
                .report
                .unsupported
                .iter()
                .any(|u| u.what.contains("github.sha"))
        );
        match &job(&imported.workflow, "check").steps[0] {
            StepKind::Run { run, .. } => assert!(!run.contains("${{")),
            other => panic!("expected a run step, got {other:?}"),
        }
    }

    #[test]
    fn a_list_is_not_a_workflow() {
        assert!(convert("ci.yml", "- build\n- test\n").is_err());
    }

    #[test]
    fn a_pool_key_is_the_pool_and_a_matrix_include_is_listed() {
        let yaml = "on: push\njobs:\n  build:\n    runs-on: arm64-hvf\n    strategy:\n      matrix:\n        os: [linux]\n        include:\n          - os: mac\n    steps:\n      - run: make\n";
        let imported = convert("build.yml", yaml).expect("the file converts");
        assert_eq!(job(&imported.workflow, "build").runs_on, "arm64-hvf");
        assert!(
            imported
                .report
                .unsupported
                .iter()
                .any(|u| u.at == "jobs.build.strategy.matrix.include")
        );
    }
}
