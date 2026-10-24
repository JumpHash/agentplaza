//! A project's workflows (PLAN 21, 23): the pipelines a project declares in its repo, `.plaza/workflows/<name>.toml`.
//!
//! A workflow names its triggers (`push`, `land`, `release.candidate`, `schedule` with a cron, `manual`) and its jobs. A
//! job has `needs` (the jobs it waits for), an optional `runs_on` (a pool, `any` by default) and an optional `matrix`, and
//! a list of steps (`[[jobs.steps]]`), each of a kind:
//!
//! | kind | what it does |
//! |---|---|
//! | `run` | `run = "cmd"` in the job's fork, with `env` and `timeout` (seconds) |
//! | `test` | the project's tests: `select = "changed"` (the coverage map's selection, the default) or `"full"` |
//! | `build` | the project's build commands, in the job's fork |
//! | `deploy` | `environment = "..."` through the deploy path; `ephemeral = true` makes a short-lived instance instead (`ttl_minutes`, `keep`) |
//! | `against` | `run = "cmd"` against the run's latest ephemeral instance (its URL is `${{ env.url }}`) |
//! | `promote` | the release (`version`, default the run's) to an `environment` |
//! | `teardown` | takes down the run's ephemeral instances now (they are taken down at the end of the run unless `keep`) |
//! | `approval` | waits for the owner, as a land approval does |
//!
//! Expressions are `${{ matrix.<key> }}`, `${{ env.url }}`, `${{ release.version }}`, `${{ change }}`, and
//! `${{ secrets.<name> }}` in an `env` value (resolved on the hub from the vault, never written to the run's record).
//!
//! A workflow compiles to a recipe (`compile`): each step is a recipe step with its [`Work`], a job's steps are a chain of
//! edges, a `needs` is an edge from the needed job's last step, and a matrix expands into one job per combination, all
//! parallel. The recipe engine runs it (`drive.rs`).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::recipes::model::{Edge, Recipe, Scope, Step};

/// Where a project's workflows live in its repo.
pub const DIR: &str = ".plaza/workflows";
/// The recipe engine's engine name for a workflow step.
pub const ENGINE: &str = "workflow";
/// The triggers a workflow may name.
pub const TRIGGERS: &[&str] = &["push", "land", "release.candidate", "schedule", "manual"];

/// A workflow as its file says it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Workflow {
    /// The workflow's name (its file's stem when the file does not say).
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default)]
    pub triggers: Vec<String>,
    /// The cron expression of a `schedule` trigger (five fields, UTC).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cron: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<Job>,
}

/// One job of a workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
    /// The pool the job's fork runs on: `any`, or a pool key of the project's stack (`arm64-hvf`). A template may name a
    /// matrix value (`${{ matrix.os }}`).
    #[serde(default = "any")]
    pub runs_on: String,
    /// Each key's values; the job expands into one job per combination.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub matrix: BTreeMap<String, Vec<Scalar>>,
    #[serde(default, rename = "steps", skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<StepKind>,
}

fn any() -> String {
    "any".into()
}

/// A matrix value: text, or a number or a boolean (written without quotes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Scalar {
    Text(String),
    Int(i64),
    Bool(bool),
}

impl Scalar {
    pub fn text(&self) -> String {
        match self {
            Scalar::Text(s) => s.clone(),
            Scalar::Int(i) => i.to_string(),
            Scalar::Bool(b) => b.to_string(),
        }
    }
}

/// A step of a job, by its kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StepKind {
    Run {
        run: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        env: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
    },
    Test {
        #[serde(default = "changed")]
        select: String,
    },
    Build {},
    Deploy {
        environment: String,
        #[serde(default, skip_serializing_if = "is_false")]
        ephemeral: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ttl_minutes: Option<u32>,
        #[serde(default, skip_serializing_if = "is_false")]
        keep: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
    Against {
        run: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        env: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<u64>,
    },
    Promote {
        environment: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
    Teardown {},
    Approval {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        message: String,
    },
}

fn changed() -> String {
    "changed".into()
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl StepKind {
    /// The kind's name, as the file writes it.
    pub fn name(&self) -> &'static str {
        match self {
            StepKind::Run { .. } => "run",
            StepKind::Test { .. } => "test",
            StepKind::Build {} => "build",
            StepKind::Deploy { .. } => "deploy",
            StepKind::Against { .. } => "against",
            StepKind::Promote { .. } => "promote",
            StepKind::Teardown {} => "teardown",
            StepKind::Approval { .. } => "approval",
        }
    }

    /// The text fields that may hold expressions, with whether a secret may be read in them (env values only).
    fn texts(&self) -> Vec<(&str, bool)> {
        match self {
            StepKind::Run { run, env, .. } | StepKind::Against { run, env, .. } => {
                let mut out = vec![(run.as_str(), false)];
                out.extend(env.values().map(|v| (v.as_str(), true)));
                out
            }
            StepKind::Deploy {
                environment,
                version,
                ..
            }
            | StepKind::Promote {
                environment,
                version,
            } => {
                let mut out = vec![(environment.as_str(), false)];
                if let Some(v) = version {
                    out.push((v.as_str(), false));
                }
                out
            }
            _ => Vec::new(),
        }
    }

    /// The timeout the step's command gets, in seconds.
    pub fn timeout_s(&self) -> u64 {
        match self {
            StepKind::Run { timeout, .. } | StepKind::Against { timeout, .. } => {
                timeout.unwrap_or(crate::base_recipe::STEP_TIMEOUT_S)
            }
            _ => crate::base_recipe::STEP_TIMEOUT_S,
        }
    }
}

/// What the checks of a workflow read from the project: its environments and its pools (`None`: not known, and not
/// checked).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Known {
    pub environments: Option<Vec<String>>,
    pub pools: Option<Vec<String>>,
}

/// A step's work in the recipe: the job it belongs to (with its matrix values in its name) and the step itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Work {
    /// The job's name in the run (`e2e[os=linux]` for a matrix expansion).
    pub job: String,
    /// The pool the job's fork runs on, with the matrix values filled in.
    pub runs_on: String,
    /// The job's matrix values for this expansion.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub matrix: BTreeMap<String, String>,
    pub step: StepKind,
}

/// One job after its matrix expansion: its name in the run, its needs (the expanded jobs it waits for) and its steps,
/// each with the recipe id it runs under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expanded {
    pub name: String,
    pub job: String,
    pub runs_on: String,
    pub matrix: BTreeMap<String, String>,
    pub needs: Vec<String>,
    pub steps: Vec<(String, StepKind)>,
}

impl Workflow {
    /// Parses a workflow's file (TOML) and checks it. `stem` names it when the file does not.
    pub fn parse(stem: &str, text: &str) -> Result<Workflow, String> {
        let mut wf: Workflow = toml::from_str(text).map_err(|e| format!("workflow {stem}: {e}"))?;
        if wf.name.trim().is_empty() {
            wf.name = stem.to_string();
        }
        wf.check(&Known::default())?;
        Ok(wf)
    }

    /// The file's text: the workflow as `.plaza/workflows/<name>.toml` holds it.
    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// Whether the workflow runs on `trigger`.
    pub fn runs_on_trigger(&self, trigger: &str) -> bool {
        self.triggers.iter().any(|t| t == trigger)
    }

    /// The workflow must make sense before it runs: its name and triggers, its jobs (unique, with needs that exist and no
    /// cycle, each with steps), its expressions and its environments and pools as `known` gives them.
    pub fn check(&self, known: &Known) -> Result<(), String> {
        let who = &self.name;
        if who.trim().is_empty() || !valid_name(who) {
            return Err(format!(
                "a workflow's name is letters, digits, dashes and underscores: {who:?}"
            ));
        }
        if self.jobs.is_empty() {
            return Err(format!("workflow {who} has no jobs"));
        }
        for trigger in &self.triggers {
            if !TRIGGERS.contains(&trigger.as_str()) {
                return Err(format!(
                    "workflow {who}: trigger {trigger} is not one of {}",
                    TRIGGERS.join(", ")
                ));
            }
        }
        let scheduled = self.triggers.iter().any(|t| t == "schedule");
        if scheduled && self.cron.trim().is_empty() {
            return Err(format!(
                "workflow {who}: a schedule trigger needs a cron expression"
            ));
        }
        if !scheduled && !self.cron.trim().is_empty() {
            return Err(format!(
                "workflow {who}: a cron expression needs the schedule trigger"
            ));
        }
        if !self.cron.trim().is_empty() {
            crate::triggers::cron::parse(&self.cron)
                .map_err(|e| format!("workflow {who}: cron {}: {e}", self.cron))?;
        }
        let mut ids = BTreeSet::new();
        for job in &self.jobs {
            if !valid_name(&job.id) {
                return Err(format!(
                    "workflow {who}: job id {:?} is letters, digits, dashes and underscores",
                    job.id
                ));
            }
            if !ids.insert(job.id.as_str()) {
                return Err(format!("workflow {who}: job {} appears twice", job.id));
            }
        }
        for job in &self.jobs {
            if job.steps.is_empty() {
                return Err(format!("workflow {who}: job {} has no steps", job.id));
            }
            for need in &job.needs {
                if !ids.contains(need.as_str()) {
                    return Err(format!(
                        "workflow {who}: job {} needs {need}, which is not a job",
                        job.id
                    ));
                }
                if need == &job.id {
                    return Err(format!("workflow {who}: job {} needs itself", job.id));
                }
            }
            for (key, values) in &job.matrix {
                if !valid_name(key) || values.is_empty() {
                    return Err(format!(
                        "workflow {who}: job {} matrix {key} needs a name and at least one value",
                        job.id
                    ));
                }
            }
            for step in &job.steps {
                check_step(who, job, step, known)?;
            }
            check_template(who, job, &job.runs_on, false)?;
            if let Some(pools) = &known.pools {
                for (_, matrix) in expand_job(job) {
                    let pool = render_plain(&job.runs_on, &matrix);
                    if !pools.contains(&pool) {
                        return Err(format!(
                            "workflow {who}: job {} runs on {pool}, which the project's stack does not declare",
                            job.id
                        ));
                    }
                }
            }
        }
        cycle_check(self)?;
        Ok(())
    }

    /// The expansion of the jobs: one per matrix combination, in the file's order.
    pub fn expand(&self) -> Vec<Expanded> {
        let mut out = Vec::new();
        for job in &self.jobs {
            for (name, matrix) in expand_job(job) {
                let needs = job
                    .needs
                    .iter()
                    .flat_map(|need| {
                        self.jobs
                            .iter()
                            .filter(move |j| &j.id == need)
                            .flat_map(|j| expand_job(j).into_iter().map(|(n, _)| n))
                    })
                    .collect();
                let steps = job
                    .steps
                    .iter()
                    .enumerate()
                    .map(|(i, step)| (format!("{name}:{}", i + 1), step.clone()))
                    .collect();
                out.push(Expanded {
                    name: name.clone(),
                    job: job.id.clone(),
                    runs_on: render_plain(&job.runs_on, &matrix),
                    matrix,
                    needs,
                    steps,
                });
            }
        }
        out
    }

    /// The recipe the workflow runs as: a step per job step (its id is `<job>:<n>`), a chain of `passed` edges through each
    /// job, and a `passed` edge from each needed job's last step to a job's first. A job with a `needs` waits for them.
    pub fn compile(&self) -> Result<Recipe, String> {
        let expanded = self.expand();
        let mut steps = Vec::new();
        let mut edges = Vec::new();
        let mut first: BTreeMap<String, String> = BTreeMap::new();
        let mut last: BTreeMap<String, String> = BTreeMap::new();
        for job in &expanded {
            for (i, (id, kind)) in job.steps.iter().enumerate() {
                steps.push(Step {
                    id: id.clone(),
                    engine: Some(ENGINE.into()),
                    gate: if matches!(kind, StepKind::Approval { .. }) {
                        vec!["owner".into()]
                    } else {
                        Vec::new()
                    },
                    work: Some(Work {
                        job: job.name.clone(),
                        runs_on: job.runs_on.clone(),
                        matrix: job.matrix.clone(),
                        step: kind.clone(),
                    }),
                    budget_share: 1.0,
                    ..Step::default()
                });
                if i == 0 {
                    first.insert(job.name.clone(), id.clone());
                } else {
                    edges.push(passed(&job.steps[i - 1].0, id));
                }
                last.insert(job.name.clone(), id.clone());
            }
        }
        for job in &expanded {
            for need in &job.needs {
                let (Some(from), Some(to)) = (last.get(need), first.get(&job.name)) else {
                    return Err(format!(
                        "workflow {}: job {} needs {need}, which has no steps",
                        self.name, job.name
                    ));
                };
                edges.push(passed(from, to));
            }
        }
        let recipe = Recipe {
            id: format!("workflow:{}", self.name),
            name: self.name.clone(),
            description: self.description.clone(),
            version: 1,
            scope: Scope::Project,
            inputs: Vec::new(),
            outputs: Vec::new(),
            triggers: self.triggers.clone(),
            steps,
            edges,
        };
        recipe
            .check()
            .map_err(|e| format!("workflow {}: {e}", self.name))?;
        Ok(recipe)
    }
}

fn passed(from: &str, to: &str) -> Edge {
    Edge {
        from: from.to_string(),
        to: to.to_string(),
        when: "passed".into(),
        max: None,
    }
}

/// A name a job or a workflow may have: letters, digits, dashes and underscores.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The expansions of a job: its name in the run and its matrix values, one per combination (the job itself when it has no
/// matrix). Keys are taken in order, so the names are stable.
pub fn expand_job(job: &Job) -> Vec<(String, BTreeMap<String, String>)> {
    let mut combos: Vec<BTreeMap<String, String>> = vec![BTreeMap::new()];
    for (key, values) in &job.matrix {
        let mut next = Vec::new();
        for combo in &combos {
            for value in values {
                let mut c = combo.clone();
                c.insert(key.clone(), value.text());
                next.push(c);
            }
        }
        combos = next;
    }
    combos
        .into_iter()
        .map(|combo| {
            let name = if combo.is_empty() {
                job.id.clone()
            } else {
                let parts: Vec<String> = combo.iter().map(|(k, v)| format!("{k}={v}")).collect();
                format!("{}[{}]", job.id, parts.join(","))
            };
            (name, combo)
        })
        .collect()
}

/// The expressions of a text: each `${{ ... }}` body, trimmed.
pub fn expressions(text: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("${{") {
        let after = &rest[start + 3..];
        let end = after
            .find("}}")
            .ok_or_else(|| format!("`{text}` has an expression that does not close"))?;
        out.push(after[..end].trim().to_string());
        rest = &after[end + 2..];
    }
    Ok(out)
}

/// Fills a template with a job's matrix values (for a pool name, at expansion).
fn render_plain(template: &str, matrix: &BTreeMap<String, String>) -> String {
    let mut out = template.to_string();
    for (key, value) in matrix {
        out = out.replace(&format!("${{{{ matrix.{key} }}}}"), value);
    }
    out
}

fn check_template(who: &str, job: &Job, text: &str, secrets: bool) -> Result<(), String> {
    for expr in expressions(text)? {
        known_expression(&expr, job, secrets)
            .map_err(|e| format!("workflow {who}: job {}: {e}", job.id))?;
    }
    Ok(())
}

/// Whether an expression is one the runs fill: `matrix.<key>` of the job, `env.url`, `release.version`, `change`, or
/// `secrets.<name>` (in an env value only).
pub fn known_expression(expr: &str, job: &Job, secrets: bool) -> Result<(), String> {
    match expr {
        "env.url" | "release.version" | "change" => Ok(()),
        _ => {
            if let Some(key) = expr.strip_prefix("matrix.") {
                if job.matrix.contains_key(key) {
                    return Ok(());
                }
                return Err(format!("${{{{ {expr} }}}} names no matrix key of the job"));
            }
            if let Some(name) = expr.strip_prefix("secrets.") {
                if secrets && valid_name(name) {
                    return Ok(());
                }
                return Err(format!(
                    "${{{{ {expr} }}}}: a secret is read in an env value, by its vault name"
                ));
            }
            Err(format!(
                "${{{{ {expr} }}}} is not an expression (env.url, matrix.<key>, release.version, change, secrets.<name>)"
            ))
        }
    }
}

fn check_step(who: &str, job: &Job, step: &StepKind, known: &Known) -> Result<(), String> {
    let at = |what: &str| {
        format!(
            "workflow {who}: job {} step {}: {what}",
            job.id,
            step.name()
        )
    };
    for (text, secrets) in step.texts() {
        for expr in expressions(text).map_err(|e| at(&e))? {
            known_expression(&expr, job, secrets).map_err(|e| at(&e))?;
        }
    }
    match step {
        StepKind::Run { run, .. } | StepKind::Against { run, .. } if run.trim().is_empty() => {
            return Err(at("the command is empty"));
        }
        StepKind::Test { select } if select != "changed" && select != "full" => {
            return Err(at(&format!("select is changed or full, not {select}")));
        }
        StepKind::Deploy {
            environment,
            ephemeral,
            ttl_minutes,
            keep,
            ..
        } => {
            check_environment(environment, known).map_err(|e| at(&e))?;
            if !ephemeral && (ttl_minutes.is_some() || *keep) {
                return Err(at("ttl_minutes and keep are for an ephemeral deploy"));
            }
        }
        StepKind::Promote { environment, .. } => {
            check_environment(environment, known).map_err(|e| at(&e))?;
        }
        StepKind::Run { env, .. } | StepKind::Against { env, .. } => {
            for name in env.keys() {
                if !valid_name(name) {
                    return Err(at(&format!(
                        "env name {name:?} is letters, digits and underscores"
                    )));
                }
            }
        }
        _ => {}
    }
    if let Some(timeout) = match step {
        StepKind::Run { timeout, .. } | StepKind::Against { timeout, .. } => *timeout,
        _ => None,
    } && timeout == 0
    {
        return Err(at("a timeout is at least one second"));
    }
    Ok(())
}

fn check_environment(name: &str, known: &Known) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("it names an environment".into());
    }
    if let Some(envs) = &known.environments
        && !envs.iter().any(|e| e == name)
    {
        return Err(format!(
            "environment {name} is not one of the project's environments"
        ));
    }
    Ok(())
}

/// The `needs` of the jobs must make no cycle: each job is placed after the jobs it needs.
fn cycle_check(wf: &Workflow) -> Result<(), String> {
    let mut placed: BTreeSet<&str> = BTreeSet::new();
    while placed.len() < wf.jobs.len() {
        let before = placed.len();
        for job in &wf.jobs {
            if !placed.contains(job.id.as_str())
                && job.needs.iter().all(|n| placed.contains(n.as_str()))
            {
                placed.insert(&job.id);
            }
        }
        if placed.len() == before {
            let stuck: Vec<&str> = wf
                .jobs
                .iter()
                .map(|j| j.id.as_str())
                .filter(|id| !placed.contains(id))
                .collect();
            return Err(format!(
                "workflow {}: the needs of {} make a cycle",
                wf.name,
                stuck.join(", ")
            ));
        }
    }
    Ok(())
}
