//! The Lab's jobs tab (`LabJobs`): every job the plaza has recorded, newest first, with filters by
//! status, recipe and mode and a search by id; a table or cards view; and the New job form under it.
//! The form has the web's fields in the web's order and builds the same job document
//! (`apps/web/src/views/lab-jobs-form.ts`): the same checks, the same keys, the same messages.
//! "Preview plan" is the dry run; "Submit" unlocks only for the document the preview was made from,
//! and asks for a confirm before it dispatches. Rows open `LabJobDetail(id)`. The list reloads on the
//! console's poll tick, and on a `training` frame at most once a second.

use std::collections::BTreeSet;
use std::fmt;
use std::time::{Duration, Instant};

use iced::widget::operation::snap_to;
use iced::widget::scrollable::RelativeOffset;
use iced::widget::{
    Id as WidgetId, Space, button, checkbox, column, container, pick_list, row, text_editor,
    text_input,
};
use iced::{Alignment, Border, Color, Element, Length, Subscription, Task, Theme};
use serde_json::Value;

use super::Id;
use super::lab::{self, Section};
use super::lab_common;
use crate::console::lab_api::{
    self, Dataset, DeviceMachine, DeviceRequest, JobDocument, JobPlan, JobSummary, JobView, Recipe,
    SubmitError,
};
use crate::console::{
    Ctx,
    client::{self, Client},
    ui,
};
use crate::theme::{self, SANS};

/// The backends a device pin may name, in the planner's order.
const BACKENDS: [&str; 5] = ["cuda", "rocm", "xpu", "mps", "cpu"];
/// The planner's modes; `None` is the planner's own choice.
const MODE_LABELS: [(&str, Option<&str>); 4] = [
    ("Auto (the planner chooses)", None),
    ("single", Some("single")),
    ("tight", Some("tight")),
    ("loose", Some("loose")),
];
/// A job's row or card eases in for this long.
const ANIM: Duration = Duration::from_millis(450);
/// A `training` frame reloads the list at most this often.
const LIVE_EVERY: Duration = Duration::from_secs(1);
const SCROLL: &str = "lab-jobs-scroll";
/// The job cards per row, as the web's `.lab-cards` grid at the page's width.
const CARDS_PER_ROW: usize = 3;

// ---- Options and filters ----

/// A pick-list entry: a value and what it reads as.
#[derive(Debug, Clone, PartialEq)]
pub struct Opt<T> {
    value: T,
    label: String,
}

impl<T> fmt::Display for Opt<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

/// A job's status filter: all, or one status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    All,
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

const STATUSES: [(Status, &str); 6] = [
    (Status::All, "All"),
    (Status::Queued, "Queued"),
    (Status::Running, "Running"),
    (Status::Done, "Done"),
    (Status::Failed, "Failed"),
    (Status::Cancelled, "Cancelled"),
];

impl Status {
    fn key(self) -> &'static str {
        match self {
            Status::All => "all",
            Status::Queued => "queued",
            Status::Running => "running",
            Status::Done => "done",
            Status::Failed => "failed",
            Status::Cancelled => "cancelled",
        }
    }

    fn matches(self, status: &str) -> bool {
        self == Status::All || self.key() == status
    }
}

/// How the list is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    #[default]
    Table,
    Cards,
}

fn tone_of(status: &str) -> ui::Tone {
    match status {
        "queued" => ui::Tone::Info,
        "running" => ui::Tone::Signal,
        "done" => ui::Tone::Ok,
        "failed" => ui::Tone::Danger,
        _ => ui::Tone::Muted,
    }
}

fn is_live(status: &str) -> bool {
    status == "queued" || status == "running"
}

/// How long a job has run: to now while it is queued or running, else to its last update.
fn duration_of(job: &JobSummary, now: u64) -> u64 {
    let end = if is_live(&job.status) {
        now
    } else {
        job.updated_ms
    };
    end.saturating_sub(job.created_ms)
}

// ---- Messages and state ----

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    /// The loaded lists (each one may have failed on its own).
    Loaded(Snapshot),
    /// A `training` frame from the hub's stream (every job's).
    Live(lab_common::TrainingFrame),
    /// An animation frame while a row is easing in.
    Frame,
    Filter(Status),
    View(View),
    Query(String),
    RecipeFilter(Opt<Option<String>>),
    ModeFilter(Opt<Option<String>>),
    ClearFilters,
    ScrollToForm,
    PickRecipe(Opt<String>),
    ToggleDataset(String, bool),
    Search(text_editor::Action),
    Budget(String),
    MinDevices(String),
    MaxDevices(String),
    MinMemory(String),
    ToggleBackend(&'static str, bool),
    PickMode(Opt<Option<String>>),
    ToggleMachine(String, bool),
    AllowCpu(bool),
    Train(text_editor::Action),
    Preview,
    /// A dry run's answer, for the document whose key it was asked with.
    Planned(String, Result<JobPlan, String>),
    AskSubmit,
    CancelConfirm,
    ConfirmSubmit,
    Submitted(Result<JobView, SubmitError>),
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    jobs: Result<Vec<JobSummary>, String>,
    recipes: Result<Vec<Recipe>, String>,
    datasets: Result<Vec<Dataset>, String>,
    machines: Result<Vec<DeviceMachine>, String>,
}

/// What the form holds. The search and the training overrides are editors, as they take many lines.
#[derive(Debug, Default)]
pub struct Form {
    recipe: Option<String>,
    datasets: BTreeSet<String>,
    search: text_editor::Content,
    budget: String,
    min_devices: String,
    max_devices: String,
    min_memory: String,
    backends: BTreeSet<String>,
    mode: Option<String>,
    machines: BTreeSet<String>,
    allow_cpu: bool,
    train: text_editor::Content,
}

/// A dry run's plan, with the key of the document it was made from.
#[derive(Debug)]
struct Preview {
    key: String,
    plan: JobPlan,
}

#[derive(Debug, Default)]
pub struct State {
    loaded: bool,
    busy: bool,
    pending: bool,
    last_load: Option<Instant>,
    jobs: Vec<JobSummary>,
    error: Option<String>,
    recipes: Vec<Recipe>,
    datasets: Vec<Dataset>,
    machines: Vec<DeviceMachine>,
    options_error: Option<String>,
    seen: BTreeSet<String>,
    fresh: Vec<(String, Instant)>,
    status: Status,
    view: View,
    recipe_filter: Option<String>,
    mode_filter: Option<String>,
    query: String,
    form: Form,
    attempted: bool,
    previewing: bool,
    submitting: bool,
    confirming: bool,
    preview: Option<Preview>,
    preview_error: Option<(String, String)>,
    /// The refused or failed submit: its message, and the job id when the job was recorded.
    submit_error: Option<(String, Option<String>)>,
    /// Set by the `APZ_DIALOG=plan` capture hook: prefill the form and run its preview on the first load.
    capture_plan: bool,
}

/// The capture hook (`APZ_VIEW=cards`): the list opens in its cards view.
pub fn capture_cards(state: &mut State) {
    state.view = View::Cards;
}

/// The capture hook (`APZ_DIALOG=plan`): the form opens prefilled and previews once the list has loaded.
pub fn capture_plan(state: &mut State) {
    state.capture_plan = true;
}

// ---- The job document ----

/// A JSON object from text. Blank text is no object, not an error.
fn parse_object(text: &str, what: &str) -> Result<Option<Value>, String> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    match serde_json::from_str::<Value>(t) {
        Ok(value @ Value::Object(_)) => Ok(Some(value)),
        Ok(_) => Err(format!("{what} must be a JSON object.")),
        Err(err) => Err(format!("{what} is not valid JSON: {err}")),
    }
}

/// The search space: a JSON object, or one `key: value` per line (a value is JSON when it parses).
fn parse_search(text: &str) -> Result<Option<Value>, String> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    if t.starts_with('{') {
        return parse_object(t, "Search space");
    }
    let mut pairs = serde_json::Map::new();
    for (i, raw) in t.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let at = line.find(':').filter(|at| *at > 0);
        let Some(at) = at else {
            return Err(format!("Search line {} needs \"key: value\".", i + 1));
        };
        let key = line[..at].trim();
        if key.is_empty() {
            return Err(format!("Search line {} needs \"key: value\".", i + 1));
        }
        if pairs.contains_key(key) {
            return Err(format!("Search key \"{key}\" is set twice."));
        }
        let value = line[at + 1..].trim();
        let parsed = serde_json::from_str::<Value>(value)
            .unwrap_or_else(|_| Value::String(value.to_string()));
        pairs.insert(key.to_string(), parsed);
    }
    Ok(Some(Value::Object(pairs)))
}

/// A whole number from a field: `None` when the field is blank, an error when it is not one at `min` or more.
fn count(text: &str, message: &str, min: u64) -> Result<Option<u64>, String> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    match t.parse::<u64>() {
        Ok(n) if n >= min && t.chars().all(|c| c.is_ascii_digit()) => Ok(Some(n)),
        _ => Err(message.to_string()),
    }
}

/// The `devices` block: only the keys the form sets, in the order the apid's struct lists them.
fn build_devices(form: &Form) -> Result<Option<DeviceRequest>, String> {
    let min = count(
        &form.min_devices,
        "Min devices is a whole number, 1 or more.",
        1,
    )?;
    let max = count(
        &form.max_devices,
        "Max devices is a whole number, 1 or more.",
        1,
    )?;
    let mem = count(
        &form.min_memory,
        "Min memory is a whole number of MiB, 1 or more.",
        1,
    )?;
    if let (Some(lo), Some(hi)) = (min, max)
        && hi < lo
    {
        return Err("Max devices is below min devices.".to_string());
    }
    let too_big = || "Devices is a number too large for the planner.".to_string();
    let mut req = DeviceRequest {
        min_devices: min
            .map(|n| u32::try_from(n).map_err(|_| too_big()))
            .transpose()?,
        max_devices: max
            .map(|n| u32::try_from(n).map_err(|_| too_big()))
            .transpose()?,
        min_memory_mib: mem,
        ..DeviceRequest::default()
    };
    let backends: Vec<String> = BACKENDS
        .iter()
        .filter(|b| form.backends.contains(**b))
        .map(|b| b.to_string())
        .collect();
    if !backends.is_empty() {
        req.backends = Some(backends);
    }
    req.mode = form.mode.clone();
    if !form.machines.is_empty() {
        req.machines = Some(form.machines.iter().cloned().collect());
    }
    if form.allow_cpu {
        req.allow_cpu = Some(true);
    }
    let empty = req.min_devices.is_none()
        && req.max_devices.is_none()
        && req.min_memory_mib.is_none()
        && req.backends.is_none()
        && req.mode.is_none()
        && req.machines.is_none()
        && req.allow_cpu.is_none();
    Ok(if empty { None } else { Some(req) })
}

/// The job document from the form, or the list of what is missing. The checks and their order are the web's.
pub fn build(form: &Form) -> Result<JobDocument, Vec<String>> {
    let mut errors = Vec::new();
    if form.recipe.is_none() {
        errors.push("Pick a vetted recipe.".to_string());
    }
    if form.datasets.is_empty() {
        errors.push("Pick at least one dataset.".to_string());
    }
    let budget = form
        .budget
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|b| b.is_finite() && *b > 0.0);
    if budget.is_none() {
        errors.push("Budget is a number of GPU-hours above 0.".to_string());
    }
    let search = parse_search(&form.search.text());
    if let Err(e) = &search {
        errors.push(e.clone());
    }
    let devices = build_devices(form);
    if let Err(e) = &devices {
        errors.push(e.clone());
    }
    let train = parse_object(&form.train.text(), "Train overrides");
    if let Err(e) = &train {
        errors.push(e.clone());
    }
    let (Some(recipe), Some(budget), Ok(search), Ok(devices), Ok(train)) =
        (form.recipe.clone(), budget, search, devices, train)
    else {
        return Err(errors);
    };
    Ok(JobDocument {
        recipe,
        datasets: form.datasets.iter().cloned().collect(),
        search,
        budget_gpu_hours: budget,
        devices,
        train,
        ..JobDocument::default()
    })
}

/// The key of the document the form describes now, when it is complete.
fn current_key(form: &Form) -> Option<String> {
    build(form)
        .ok()
        .and_then(|doc| serde_json::to_string(&doc).ok())
}

/// The preview is fresh when it was made from the document the form describes now.
fn fresh(state: &State) -> bool {
    match (&state.preview, current_key(&state.form)) {
        (Some(p), Some(key)) => p.key == key,
        _ => false,
    }
}

// ---- Screen wiring ----

/// The navigation a message asks for.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// The poll tick's load: the jobs, the recipes, the datasets and the fleet's devices, one call each.
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    if state.busy {
        state.pending = true;
        return Task::none();
    }
    state.busy = true;
    state.last_load = Some(Instant::now());
    let c = ctx.client.clone();
    Task::perform(async move { fetch(&c) }, Msg::Loaded)
}

fn fetch(c: &Client) -> Snapshot {
    Snapshot {
        jobs: lab_api::list_jobs(c),
        recipes: lab_api::list_recipes(c),
        datasets: lab_api::list_datasets(c),
        machines: lab_api::list_devices(c).map(|v| v.machines),
    }
}

/// The live `training` frames (for a reload) and the row animation's frames (while a row eases in).
pub fn subscription(state: &State, ctx: &Ctx) -> Subscription<Msg> {
    let live = lab_common::live(&ctx.client)
        .filter_map(|e| lab_common::training_frame(&e))
        .map(Msg::Live);
    let animating = if state.fresh.is_empty() {
        Subscription::none()
    } else {
        iced::time::every(Duration::from_millis(33)).map(|_| Msg::Frame)
    };
    Subscription::batch([live, animating])
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(snap) => {
            state.busy = false;
            match snap.jobs {
                Ok(jobs) => {
                    let ids: BTreeSet<String> = jobs.iter().map(|j| j.id.clone()).collect();
                    if state.loaded {
                        let now = Instant::now();
                        let arrived: Vec<(String, Instant)> = ids
                            .difference(&state.seen)
                            .map(|id| (id.clone(), now))
                            .collect();
                        state.fresh.extend(arrived);
                    }
                    state.seen = ids;
                    state.jobs = jobs;
                    state.loaded = true;
                    state.error = None;
                }
                Err(e) => state.error = Some(e),
            }
            let mut failed = Vec::new();
            match snap.recipes {
                Ok(recipes) => state.recipes = recipes,
                Err(e) => failed.push(format!("Recipes: {e}")),
            }
            match snap.datasets {
                Ok(datasets) => state.datasets = datasets,
                Err(e) => failed.push(format!("Datasets: {e}")),
            }
            match snap.machines {
                Ok(machines) => state.machines = machines,
                Err(e) => failed.push(format!("Devices: {e}")),
            }
            state.options_error = (!failed.is_empty()).then(|| failed.join(" "));
            let mut tasks = Vec::new();
            if state.pending {
                state.pending = false;
                tasks.push(load(state, ctx));
            }
            if state.capture_plan && state.loaded {
                state.capture_plan = false;
                state.form.recipe = Some("rung-predictor@1".to_string());
                state
                    .form
                    .datasets
                    .insert("ds-afa92f9f780f5083".to_string());
                state.form.budget = "0.5".to_string();
                tasks.push(Task::done(Msg::Preview));
            }
            Task::batch(tasks)
        }
        Msg::Live(_) => {
            let quiet = state.last_load.is_none_or(|t| t.elapsed() >= LIVE_EVERY);
            if state.loaded && !state.busy && quiet {
                load(state, ctx)
            } else {
                Task::none()
            }
        }
        Msg::Frame => {
            state.fresh.retain(|(_, at)| at.elapsed() < ANIM);
            Task::none()
        }
        Msg::Filter(status) => {
            state.status = status;
            Task::none()
        }
        Msg::View(view) => {
            state.view = view;
            Task::none()
        }
        Msg::Query(query) => {
            state.query = query;
            Task::none()
        }
        Msg::RecipeFilter(choice) => {
            state.recipe_filter = choice.value;
            Task::none()
        }
        Msg::ModeFilter(choice) => {
            state.mode_filter = choice.value;
            Task::none()
        }
        Msg::ClearFilters => {
            state.status = Status::All;
            state.recipe_filter = None;
            state.mode_filter = None;
            state.query.clear();
            Task::none()
        }
        Msg::ScrollToForm => snap_to(WidgetId::new(SCROLL), RelativeOffset::END),
        Msg::PickRecipe(choice) => {
            state.form.recipe = Some(choice.value);
            Task::none()
        }
        Msg::ToggleDataset(id, on) => {
            set_member(&mut state.form.datasets, id, on);
            Task::none()
        }
        Msg::Search(action) => {
            state.form.search.perform(action);
            Task::none()
        }
        Msg::Budget(text) => {
            state.form.budget = text;
            Task::none()
        }
        Msg::MinDevices(text) => {
            state.form.min_devices = text;
            Task::none()
        }
        Msg::MaxDevices(text) => {
            state.form.max_devices = text;
            Task::none()
        }
        Msg::MinMemory(text) => {
            state.form.min_memory = text;
            Task::none()
        }
        Msg::ToggleBackend(name, on) => {
            set_member(&mut state.form.backends, name.to_string(), on);
            Task::none()
        }
        Msg::PickMode(choice) => {
            state.form.mode = choice.value;
            Task::none()
        }
        Msg::ToggleMachine(id, on) => {
            set_member(&mut state.form.machines, id, on);
            Task::none()
        }
        Msg::AllowCpu(on) => {
            state.form.allow_cpu = on;
            Task::none()
        }
        Msg::Train(action) => {
            state.form.train.perform(action);
            Task::none()
        }
        Msg::Preview => {
            state.attempted = true;
            let Ok(doc) = build(&state.form) else {
                return Task::none();
            };
            let Ok(key) = serde_json::to_string(&doc) else {
                return Task::none();
            };
            state.previewing = true;
            state.preview_error = None;
            state.submit_error = None;
            let c = ctx.client.clone();
            client::task(
                move || lab_api::plan_job(&c, &doc),
                move |res| Msg::Planned(key.clone(), res),
            )
        }
        Msg::Planned(key, res) => {
            state.previewing = false;
            if current_key(&state.form).as_deref() == Some(key.as_str()) {
                match res {
                    Ok(plan) => {
                        state.preview = Some(Preview { key, plan });
                        state.preview_error = None;
                    }
                    Err(message) => {
                        state.preview = None;
                        state.preview_error = Some((key, message));
                    }
                }
            }
            Task::none()
        }
        Msg::AskSubmit => {
            if fresh(state) {
                state.confirming = true;
                state.submit_error = None;
            }
            Task::none()
        }
        Msg::CancelConfirm => {
            state.confirming = false;
            Task::none()
        }
        Msg::ConfirmSubmit => {
            let ready = fresh(state);
            let Ok(doc) = build(&state.form) else {
                state.confirming = false;
                return Task::none();
            };
            if !ready {
                state.confirming = false;
                return Task::none();
            }
            state.submitting = true;
            let c = ctx.client.clone();
            Task::perform(async move { lab_api::submit_job(&c, &doc) }, Msg::Submitted)
        }
        Msg::Submitted(res) => {
            state.submitting = false;
            state.confirming = false;
            match res {
                Ok(job) => return Task::done(Msg::Go(Id::LabJobDetail(job.id))),
                Err(SubmitError::Dispatch { message, job }) => {
                    state.submit_error = Some((message, Some(job.id)))
                }
                Err(SubmitError::Refused(message)) => state.submit_error = Some((message, None)),
            }
            Task::none()
        }
    }
}

/// Adds or removes `id` from a set of the form's picks.
fn set_member(set: &mut BTreeSet<String>, id: String, on: bool) {
    if on {
        set.insert(id);
    } else {
        set.remove(&id);
    }
}

// ---- Views ----

fn fade(fresh_at: &[(String, Instant)], id: &str) -> f32 {
    fresh_at
        .iter()
        .find(|(who, _)| who == id)
        .map_or(1.0, |(_, at)| {
            (at.elapsed().as_secs_f32() / ANIM.as_secs_f32()).clamp(0.0, 1.0)
        })
}

fn tinted(c: Color, alpha: f32) -> Color {
    Color {
        a: c.a * alpha,
        ..c
    }
}

/// A job's status word as the web's chip: its first letter capitalised.
fn cap(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The budget cell as the web's `budgetCell`: the used and the budget, then a meter that turns red
/// over budget (the figure too).
fn budget_cell<'a>(job: &JobSummary, alpha: f32) -> Element<'a, Msg> {
    let over = job.budget_gpu_hours > 0.0 && job.gpu_hours > job.budget_gpu_hours;
    let ratio = if job.budget_gpu_hours > 0.0 {
        job.gpu_hours / job.budget_gpu_hours
    } else {
        0.0
    };
    let figure = format!(
        "{} of {}",
        lab_common::number(job.gpu_hours, 2),
        lab_common::number(job.budget_gpu_hours, 0)
    );
    let ink = if over {
        crate::theme::palette().crit
    } else {
        tinted(crate::theme::palette().ink, alpha)
    };
    column![
        ui::sans(figure, 13.0, ink),
        container(lab_common::meter(ratio, over)).width(Length::Fixed(140.0)),
    ]
    .spacing(4)
    .width(Length::Fill)
    .into()
}

/// The cells of one job's row, in the web's table order: id, status, mode, recipe, datasets,
/// created, duration, GPU-h and error. A fresh row eases in through `alpha`.
fn job_cells<'a>(job: &JobSummary, alpha: f32, now: u64) -> Vec<Element<'a, Msg>> {
    let ink = tinted(crate::theme::palette().ink, alpha);
    let ink_2 = tinted(crate::theme::palette().ink_2, alpha);
    let ink_3 = tinted(crate::theme::palette().ink_3, alpha);
    let error: Element<'a, Msg> = match job.error.as_deref() {
        Some(message) if !message.is_empty() => ui::sans(
            message.to_string(),
            13.0,
            tinted(crate::theme::palette().crit, alpha),
        )
        .into(),
        _ => ui::sans("—", 13.0, ink_3).into(),
    };
    vec![
        ui::mono(job.id.clone(), 12.0, ink).into(),
        ui::badge(&cap(&job.status), tone_of(&job.status)),
        ui::sans(job.mode.clone(), 13.0, ink_2).into(),
        ui::mono(job.recipe.clone(), 12.0, ink_2).into(),
        ui::sans(job.datasets.len().to_string(), 13.0, ink).into(),
        ui::sans(lab_common::ago_text(job.created_ms, now), 13.0, ink_2).into(),
        ui::sans(lab_common::run_time(duration_of(job, now)), 13.0, ink).into(),
        budget_cell(job, alpha),
        error,
    ]
}

/// The columns of the jobs table, as the web's `tableOf`.
const JOB_COLS: [lab_common::Col; 9] = [
    lab_common::Col {
        head: "ID",
        portion: 3,
        right: false,
    },
    lab_common::Col {
        head: "Status",
        portion: 2,
        right: false,
    },
    lab_common::Col {
        head: "Mode",
        portion: 2,
        right: false,
    },
    lab_common::Col {
        head: "Recipe",
        portion: 3,
        right: false,
    },
    lab_common::Col {
        head: "Datasets",
        portion: 2,
        right: true,
    },
    lab_common::Col {
        head: "Created",
        portion: 2,
        right: false,
    },
    lab_common::Col {
        head: "Duration",
        portion: 2,
        right: true,
    },
    lab_common::Col {
        head: "GPU-h",
        portion: 3,
        right: false,
    },
    lab_common::Col {
        head: "Error",
        portion: 3,
        right: false,
    },
];

/// One job as the web's card: its id and status on the head, the recipe, the datasets, the times,
/// the budget and any error, the whole card a link to the job.
fn job_card<'a>(job: &JobSummary, now: u64, alpha: f32) -> Element<'a, Msg> {
    let ink_2 = tinted(crate::theme::palette().ink_2, alpha);
    let ink_3 = tinted(crate::theme::palette().ink_3, alpha);
    let datasets = job.datasets.len();
    let mut lines: Vec<Element<'a, Msg>> = vec![
        row![
            ui::sans("Recipe", 12.5, ink_3),
            ui::mono(job.recipe.clone(), 12.0, ink_2),
            ui::sans(format!("· {}", job.mode), 12.5, ink_2),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into(),
        ui::sans(
            format!(
                "{} dataset{} · created {} · {}",
                datasets,
                if datasets == 1 { "" } else { "s" },
                lab_common::ago_text(job.created_ms, now),
                lab_common::run_time(duration_of(job, now))
            ),
            12.5,
            ink_3,
        )
        .into(),
        budget_cell(job, alpha),
    ];
    if let Some(message) = job.error.as_deref().filter(|m| !m.is_empty()) {
        lines.push(
            ui::sans(
                message.to_string(),
                12.5,
                tinted(crate::theme::palette().crit, alpha),
            )
            .into(),
        );
    }
    let card = ui::card_with(
        &job.id,
        vec![ui::badge(&cap(&job.status), tone_of(&job.status))],
        column(lines).spacing(8).width(Length::Fill).into(),
    );
    button(card)
        .style(|_: &Theme, _: button::Status| button::Style {
            background: None,
            text_color: crate::theme::palette().ink,
            border: Border::default(),
            ..Default::default()
        })
        .padding(0)
        .width(Length::Fill)
        .on_press(Msg::Go(Id::LabJobDetail(job.id.clone())))
        .into()
}

fn visible(state: &State) -> Vec<&JobSummary> {
    let q = state.query.trim().to_lowercase();
    state
        .jobs
        .iter()
        .filter(|j| {
            state.status.matches(&j.status)
                && state.recipe_filter.as_ref().is_none_or(|r| &j.recipe == r)
                && state.mode_filter.as_ref().is_none_or(|m| &j.mode == m)
                && (q.is_empty() || j.id.to_lowercase().contains(&q))
        })
        .collect()
}

/// The filter chips (one per status, each with its count), the recipe and mode pickers, the search
/// by id and the Table / Cards switch, as the web's `.lab-tools`.
fn tools<'a>(state: &'a State) -> Element<'a, Msg> {
    let chips: Vec<Element<'a, Msg>> = STATUSES
        .iter()
        .map(|(s, label)| {
            let count = if *s == Status::All {
                state.jobs.len()
            } else {
                state.jobs.iter().filter(|j| j.status == s.key()).count()
            };
            lab_common::chip(label, count, state.status == *s, Msg::Filter(*s))
        })
        .collect();
    let mut recipe_names: Vec<String> = state.jobs.iter().map(|j| j.recipe.clone()).collect();
    recipe_names.sort();
    recipe_names.dedup();
    let mut recipe_options = vec![Opt {
        value: None,
        label: "All recipes".to_string(),
    }];
    recipe_options.extend(recipe_names.into_iter().map(|n| Opt {
        value: Some(n.clone()),
        label: n,
    }));
    let recipe_selected = recipe_options
        .iter()
        .find(|o| o.value == state.recipe_filter)
        .cloned();
    let recipe_pick = pick_list(recipe_options, recipe_selected, Msg::RecipeFilter)
        .style(crate::theme::pick_list)
        .width(Length::Fixed(200.0));
    let mode_options: Vec<Opt<Option<String>>> = vec![
        Opt {
            value: None,
            label: "All modes".to_string(),
        },
        Opt {
            value: Some("single".to_string()),
            label: "single".to_string(),
        },
        Opt {
            value: Some("tight".to_string()),
            label: "tight".to_string(),
        },
        Opt {
            value: Some("loose".to_string()),
            label: "loose".to_string(),
        },
    ];
    let mode_selected = mode_options
        .iter()
        .find(|o| o.value == state.mode_filter)
        .cloned();
    let mode_pick = pick_list(mode_options, mode_selected, Msg::ModeFilter)
        .style(crate::theme::pick_list)
        .width(Length::Fixed(160.0));
    let search = text_input("Search by job id", &state.query)
        .on_input(Msg::Query)
        .padding([6, 10])
        .size(13.0)
        .width(Length::Fixed(220.0))
        .style(theme::input);
    let view_seg = ui::seg(
        &[(View::Table, "Table"), (View::Cards, "Cards")],
        state.view,
        Msg::View,
    );
    let right = row![recipe_pick, mode_pick, search, view_seg]
        .spacing(8)
        .align_y(Alignment::Center)
        .wrap();
    row![
        row(chips).spacing(6).align_y(Alignment::Center).wrap(),
        right,
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .wrap()
    .into()
}

/// The jobs list under the tools: the count line, any load error, then the table, the cards, or the
/// empty state.
fn list<'a>(state: &'a State) -> Element<'a, Msg> {
    let rows = visible(state);
    let now = lab_common::now_ms();
    let fresh_at = &state.fresh;
    let total = state.jobs.len();
    let count_line = lab_common::note(if state.loaded {
        format!("{} of {} shown", rows.len(), total)
    } else {
        String::new()
    });
    let errors: Element<'a, Msg> = match state.error.as_deref() {
        Some(message) => lab_common::err(message),
        None => Space::new().into(),
    };
    let body: Element<'a, Msg> = if !state.loaded {
        lab_common::note(if state.error.is_some() {
            ""
        } else {
            "Loading jobs…"
        })
    } else if total == 0 {
        lab_common::empty(
            "flask-conical",
            "No jobs yet",
            None,
            Some(ui::btn_sm(
                "New job",
                Some("plus"),
                ui::Kind::Primary,
                Some(Msg::ScrollToForm),
            )),
        )
    } else if rows.is_empty() {
        lab_common::empty(
            "search",
            "No jobs match",
            None,
            Some(ui::btn_sm(
                "Clear filters",
                None,
                ui::Kind::Ghost,
                Some(Msg::ClearFilters),
            )),
        )
    } else if state.view == View::Table {
        let rows: Vec<lab_common::Row<'a, Msg>> = rows
            .iter()
            .map(|j| lab_common::Row {
                cells: job_cells(j, fade(fresh_at, &j.id), now),
                on_press: Some(Msg::Go(Id::LabJobDetail(j.id.clone()))),
            })
            .collect();
        lab_common::table(&JOB_COLS, rows)
    } else {
        let cards: Vec<Element<'a, Msg>> = rows
            .chunks(CARDS_PER_ROW)
            .map(|chunk| {
                let mut cells: Vec<Element<'a, Msg>> = chunk
                    .iter()
                    .map(|j| {
                        container(job_card(j, now, fade(fresh_at, &j.id)))
                            .width(Length::FillPortion(1))
                            .into()
                    })
                    .collect();
                while cells.len() < CARDS_PER_ROW {
                    cells.push(Space::new().width(Length::FillPortion(1)).into());
                }
                row(cells).spacing(12).width(Length::Fill).into()
            })
            .collect();
        column(cards).spacing(12).width(Length::Fill).into()
    };
    column![count_line, errors, body]
        .spacing(12)
        .width(Length::Fill)
        .into()
}

/// A form field: its label, its control and, when there is one, its hint (the web's `.field-l`,
/// the control, and `small.hint`).
fn field<'a>(title: &str, control: Element<'a, Msg>, hint: &str) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = vec![ui::label(title).into(), control];
    if !hint.is_empty() {
        parts.push(ui::faint(hint.to_string()).size(12.0).into());
    }
    column(parts).spacing(6).width(Length::Fill).into()
}

fn recipe_options(state: &State) -> Vec<Opt<String>> {
    state
        .recipes
        .iter()
        .filter(|r| r.vetted_by.is_some() && r.vetted_ms.is_some())
        .map(|r| Opt {
            value: r.id.clone(),
            label: format!("{} · {}", r.id, r.name),
        })
        .collect()
}

/// The recipe's hint, as the web's `recipeHint`: the recipes not vetted yet, or that none is.
fn unvetted_note(state: &State) -> String {
    let vetted = recipe_options(state).len();
    let unvetted: Vec<&str> = state
        .recipes
        .iter()
        .filter(|r| r.vetted_by.is_none() || r.vetted_ms.is_none())
        .map(|r| r.id.as_str())
        .collect();
    if vetted == 0 {
        "No vetted recipe yet. Vet one under Recipes.".to_string()
    } else if unvetted.is_empty() {
        String::new()
    } else {
        format!(
            "Not listed until vetted: {}. Vet {} under Recipes.",
            unvetted.join(", "),
            if unvetted.len() == 1 { "it" } else { "them" }
        )
    }
}

/// The datasets to pick from, as the web's `.lab-checks`: a checkbox, the name, the id, kind and rows
/// and a frozen chip; a scroll past 200 pixels.
fn dataset_checks<'a>(state: &'a State) -> Element<'a, Msg> {
    if state.datasets.is_empty() {
        return lab_common::note("No datasets yet. Make one on the Datasets tab.");
    }
    let rows: Vec<Element<'a, Msg>> = state
        .datasets
        .iter()
        .map(|d| {
            let id = d.id.clone();
            let picked = state.form.datasets.contains(&d.id);
            let mut line = row![
                checkbox(picked)
                    .label(d.name.clone())
                    .on_toggle(move |on| Msg::ToggleDataset(id.clone(), on)),
                lab_common::note(format!(
                    "{} · {} · {} rows",
                    d.id,
                    d.kind,
                    lab_common::count(d.rows)
                )),
            ]
            .spacing(10)
            .align_y(Alignment::Center);
            if d.frozen_rows > 0 {
                line = line.push(ui::badge::<Msg>("frozen", ui::Tone::Info));
            }
            line.into()
        })
        .collect();
    scroll_box(column(rows).spacing(6).width(Length::Fill).into(), 200.0)
}

/// The machines to pick from, as the web's machine checks: a checkbox per machine, an offline one
/// marked so.
fn machine_checks<'a>(state: &'a State) -> Element<'a, Msg> {
    if state.machines.is_empty() {
        return lab_common::note("No machines report devices.");
    }
    let boxes: Vec<Element<'a, Msg>> = state
        .machines
        .iter()
        .map(|m| {
            let id = m.id.clone();
            let picked = state.form.machines.contains(&m.id);
            let label = if m.online {
                m.id.clone()
            } else {
                format!("{} · offline", m.id)
            };
            checkbox(picked)
                .label(label)
                .on_toggle(move |on| Msg::ToggleMachine(id.clone(), on))
                .into()
        })
        .collect();
    row(boxes)
        .spacing(16)
        .align_y(Alignment::Center)
        .wrap()
        .into()
}

/// A bordered box that scrolls past `max` pixels, the web's `.lab-checks` scroller.
fn scroll_box<'a>(body: Element<'a, Msg>, max: f32) -> Element<'a, Msg> {
    container(ui::scroll(body).width(Length::Fill))
        .max_height(max)
        .padding(4)
        .width(Length::Fill)
        .into()
}

/// The device pin: the backends the job may run on (none picked means any).
fn backend_checks<'a>(state: &'a State) -> Element<'a, Msg> {
    let f = &state.form;
    row(BACKENDS
        .iter()
        .map(|b| {
            let name: &'static str = b;
            checkbox(f.backends.contains(*b))
                .label(*b)
                .on_toggle(move |on| Msg::ToggleBackend(name, on))
                .into()
        })
        .collect::<Vec<Element<'a, Msg>>>())
    .spacing(14)
    .align_y(Alignment::Center)
    .wrap()
    .into()
}

/// The form's cells in the web's `.lab-form` grid: four to a row, a wide cell takes a row of its own.
fn form_grid<'a>(cells: Vec<(Element<'a, Msg>, bool)>) -> Element<'a, Msg> {
    const COLS: usize = 4;
    let mut rows: Vec<Element<'a, Msg>> = Vec::new();
    let mut current: Vec<Element<'a, Msg>> = Vec::new();
    let flush = |current: &mut Vec<Element<'a, Msg>>, rows: &mut Vec<Element<'a, Msg>>| {
        if current.is_empty() {
            return;
        }
        while current.len() < COLS {
            current.push(Space::new().width(Length::FillPortion(1)).into());
        }
        rows.push(
            row(std::mem::take(current))
                .spacing(12)
                .width(Length::Fill)
                .into(),
        );
    };
    for (cell, wide) in cells {
        if wide {
            flush(&mut current, &mut rows);
            rows.push(container(cell).width(Length::Fill).into());
        } else {
            if current.len() == COLS {
                flush(&mut current, &mut rows);
            }
            current.push(container(cell).width(Length::FillPortion(1)).into());
        }
    }
    flush(&mut current, &mut rows);
    column(rows).spacing(12).width(Length::Fill).into()
}

/// The plan of a dry run as the web's `planDiagram`: a key-value list (mode, devices, inner steps,
/// staleness, rounds, frozen, islands and the output directory).
pub fn plan_view<'a, M: 'a>(plan: &JobPlan) -> Element<'a, M> {
    let backend = plan
        .islands
        .first()
        .map(|i| i.backend.clone())
        .unwrap_or_else(|| "—".to_string());
    let loose = plan.mode == "loose";
    let rounds = plan
        .rounds
        .map_or_else(|| "—".to_string(), lab_common::count);
    let mode = if loose {
        format!(
            "loose, {} rounds, staleness {}",
            lab_common::count(plan.rounds.unwrap_or(0)),
            lab_common::count(plan.staleness)
        )
    } else {
        plan.mode.clone()
    };
    let islands: Element<'a, M> = if plan.islands.is_empty() {
        ui::sans("—", 13.0, crate::theme::palette().ink).into()
    } else {
        column(
            plan.islands
                .iter()
                .map(|i| {
                    let machines = i
                        .machines
                        .iter()
                        .map(|m| {
                            format!(
                                "{} (devices {})",
                                m.id,
                                m.devices
                                    .iter()
                                    .map(|d| d.to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("; ");
                    ui::sans(
                        format!(
                            "Island {}: {}, leader {}, rendezvous {}, machines {}",
                            i.id, i.backend, i.leader, i.rdzv, machines
                        ),
                        13.0,
                        crate::theme::palette().ink,
                    )
                    .into()
                })
                .collect::<Vec<Element<'a, M>>>(),
        )
        .spacing(4)
        .into()
    };
    let pairs: Vec<(&str, Element<'a, M>)> = vec![
        (
            "Mode",
            ui::sans(mode, 13.0, crate::theme::palette().ink).into(),
        ),
        (
            "Devices",
            ui::sans(
                format!("{} on {}", lab_common::count(plan.devices_total), backend),
                13.0,
                crate::theme::palette().ink,
            )
            .into(),
        ),
        (
            "Inner steps",
            ui::sans(
                lab_common::count(plan.inner_steps),
                13.0,
                crate::theme::palette().ink,
            )
            .into(),
        ),
        (
            "Staleness",
            ui::sans(
                if loose {
                    lab_common::count(plan.staleness)
                } else {
                    "—".to_string()
                },
                13.0,
                crate::theme::palette().ink,
            )
            .into(),
        ),
        (
            "Rounds",
            ui::sans(rounds, 13.0, crate::theme::palette().ink).into(),
        ),
        (
            "Frozen",
            ui::sans(
                plan.frozen.clone().unwrap_or_else(|| "none".to_string()),
                13.0,
                crate::theme::palette().ink,
            )
            .into(),
        ),
        ("Islands", islands),
        (
            "Output dir",
            ui::mono(
                if plan.output_dir.is_empty() {
                    "—".to_string()
                } else {
                    plan.output_dir.clone()
                },
                12.0,
                crate::theme::palette().ink,
            )
            .into(),
        ),
    ];
    column(
        pairs
            .into_iter()
            .map(|(label, value)| {
                row![
                    container(ui::sans(
                        label.to_string(),
                        12.5,
                        crate::theme::palette().ink_3
                    ))
                    .width(Length::Fixed(140.0)),
                    container(value).width(Length::Fill),
                ]
                .spacing(12)
                .align_y(Alignment::Start)
                .into()
            })
            .collect::<Vec<Element<'a, M>>>(),
    )
    .spacing(8)
    .width(Length::Fill)
    .into()
}

/// The panel of a plan (the web's `.lab-confirm`): a sheet-2 ground, a summary and two actions.
fn panel<'a>(content: Element<'a, Msg>) -> Element<'a, Msg> {
    container(content)
        .padding(12)
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().sheet_2.into()),
            border: Border::default().rounded(crate::theme::skin().radius),
            ..Default::default()
        })
        .into()
}

/// The plan zone under the form, as the web's `planZone`: the preview state, the plan's list, then
/// the submit (or its confirm strip) and any submit error.
fn plan_zone<'a>(state: &'a State, fresh_now: bool, doc_ok: bool) -> Element<'a, Msg> {
    if state.previewing {
        return lab_common::note("Planning the job…");
    }
    let key = current_key(&state.form);
    if let (Some((k, message)), Some(now)) = (&state.preview_error, &key)
        && k == now
    {
        return lab_common::err(message);
    }
    let Some(p) = &state.preview else {
        return lab_common::note(if doc_ok {
            "Preview the plan to see where the job would run. Submit unlocks for the previewed plan only."
        } else {
            "Fill in the form, then preview the plan. The plan shows where the job would run."
        });
    };
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if !fresh_now {
        parts.push(lab_common::note(
            "The form changed since this preview. Preview it again to submit.",
        ));
    }
    parts.push(plan_view(&p.plan));
    if let Some(note) = submit_area(state) {
        parts.push(note);
    }
    let busy = state.submitting;
    if state.confirming && fresh_now && doc_ok {
        if let Ok(doc) = build(&state.form) {
            let n = doc.datasets.len();
            parts.push(panel(
                column![
                    ui::sans(
                        format!(
                            "Submit {} on {n} dataset{} with a budget of {} GPU-hours? The job is queued and dispatches to the fleet now.",
                            doc.recipe,
                            if n == 1 { "" } else { "s" },
                            doc.budget_gpu_hours
                        ),
                        13.0,
                        crate::theme::palette().ink,
                    ),
                    row![
                        ui::btn("Keep editing", None, ui::Kind::Ghost, Some(Msg::CancelConfirm)),
                        ui::btn(
                            "Submit job",
                            None,
                            ui::Kind::Primary,
                            (!busy).then_some(Msg::ConfirmSubmit),
                        ),
                    ]
                    .spacing(8),
                ]
                .spacing(10)
                .into(),
            ));
        }
    } else {
        parts.push(
            row![ui::btn(
                "Submit job",
                None,
                ui::Kind::Primary,
                (fresh_now && !busy && doc_ok).then_some(Msg::AskSubmit),
            )]
            .into(),
        );
    }
    column(parts).spacing(12).width(Length::Fill).into()
}

/// The submit state's line: a planner refusal, or the job recorded as failed with its link.
fn submit_area<'a>(state: &'a State) -> Option<Element<'a, Msg>> {
    if state.submitting {
        return Some(lab_common::note("Submitting…"));
    }
    match &state.submit_error {
        None => None,
        Some((message, Some(job))) => Some(
            column![
                lab_common::err(&format!("The job was recorded as failed: {message}")),
                ui::link(
                    &format!("Open job {job}"),
                    Msg::Go(Id::LabJobDetail(job.clone())),
                    true
                ),
            ]
            .spacing(6)
            .into(),
        ),
        Some((message, None)) => Some(lab_common::err(message)),
    }
}

fn form_card<'a>(state: &'a State) -> Element<'a, Msg> {
    let f = &state.form;
    let options = recipe_options(state);
    let selected = f
        .recipe
        .as_ref()
        .and_then(|id| options.iter().find(|o| &o.value == id).cloned());
    let recipe = pick_list(options, selected, Msg::PickRecipe)
        .style(crate::theme::pick_list)
        .placeholder("Choose a vetted recipe")
        .width(Length::Fill);
    let search = text_editor(&f.search)
        .style(crate::theme::editor)
        .font(SANS)
        .size(13.0)
        .height(Length::Fixed(96.0))
        .on_action(Msg::Search);
    let budget = text_input("", &f.budget)
        .on_input(Msg::Budget)
        .padding([8, 10])
        .size(13.0)
        .width(Length::Fill)
        .style(theme::input);
    let min = text_input("", &f.min_devices)
        .on_input(Msg::MinDevices)
        .padding([8, 10])
        .size(13.0)
        .width(Length::Fill)
        .style(theme::input);
    let max = text_input("", &f.max_devices)
        .on_input(Msg::MaxDevices)
        .padding([8, 10])
        .size(13.0)
        .width(Length::Fill)
        .style(theme::input);
    let mem = text_input("", &f.min_memory)
        .on_input(Msg::MinMemory)
        .padding([8, 10])
        .size(13.0)
        .width(Length::Fill)
        .style(theme::input);
    let train = text_editor(&f.train)
        .style(crate::theme::editor)
        .font(SANS)
        .size(13.0)
        .height(Length::Fixed(72.0))
        .on_action(Msg::Train);
    let mode_options: Vec<Opt<Option<String>>> = MODE_LABELS
        .iter()
        .map(|(label, value)| Opt {
            value: value.map(|v| v.to_string()),
            label: (*label).to_string(),
        })
        .collect();
    let mode_selected = mode_options.iter().find(|o| o.value == f.mode).cloned();
    let mode = pick_list(mode_options, mode_selected, Msg::PickMode)
        .style(crate::theme::pick_list)
        .width(Length::Fill);
    let allow = checkbox(f.allow_cpu)
        .label("Allow CPU")
        .on_toggle(Msg::AllowCpu);

    let errors = build(f).err().unwrap_or_default();
    let doc_ok = errors.is_empty();
    let fresh_now = fresh(state);
    let busy = state.previewing || state.submitting;
    let problems: Option<Element<'a, Msg>> = (state.attempted && !doc_ok).then(|| {
        column(
            errors
                .iter()
                .map(|e| ui::sans(e.clone(), 12.0, crate::theme::palette().warn).into())
                .collect::<Vec<Element<'a, Msg>>>(),
        )
        .spacing(4)
        .into()
    });
    let cells: Vec<(Element<'a, Msg>, bool)> = vec![
        (field("Recipe", recipe.into(), &unvetted_note(state)), false),
        (field("Datasets", dataset_checks(state), ""), true),
        (
            field(
                "Search space",
                search.into(),
                "JSON, or one key: value per line.",
            ),
            true,
        ),
        (field("Budget (GPU-h)", budget.into(), ""), false),
        (field("Min devices", min.into(), ""), false),
        (field("Max devices", max.into(), ""), false),
        (field("Min memory (MiB)", mem.into(), ""), false),
        (field("Device pin", backend_checks(state), ""), false),
        (field("Mode", mode.into(), ""), false),
        (field("Machines", machine_checks(state), ""), true),
        (
            column![ui::label("Allow CPU"), allow]
                .spacing(6)
                .width(Length::Fill)
                .into(),
            false,
        ),
        (
            field("Train overrides", train.into(), "JSON object, optional."),
            true,
        ),
    ];
    let actions = row![ui::btn(
        "Preview plan",
        Some("eye"),
        ui::Kind::Ghost,
        (!busy).then_some(Msg::Preview),
    )]
    .spacing(8)
    .align_y(Alignment::Center);
    let options_note: Element<'a, Msg> = match &state.options_error {
        Some(message) => lab_common::err(message),
        None => Space::new().into(),
    };
    let body = column![
        problems.unwrap_or_else(|| Space::new().into()),
        form_grid(cells),
        options_note,
        actions,
        plan_zone(state, fresh_now, doc_ok),
    ]
    .spacing(12)
    .width(Length::Fill);
    ui::card_with("New job", Vec::new(), body.into())
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let body = column![tools(state), list(state), form_card(state)]
        .spacing(16)
        .width(Length::Fill);
    lab::frame(
        Section::Jobs,
        ui::scroll(body)
            .id(WidgetId::new(SCROLL))
            .height(Length::Fill)
            .into(),
        Msg::Go,
    )
}
