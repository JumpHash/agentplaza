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
    Id as WidgetId, Space, button, checkbox, column, container, pick_list, row, text, text_editor,
    text_input,
};
use iced::{Alignment, Border, Color, Element, Length, Padding, Subscription, Task, Theme};
use serde_json::Value;

use super::Id;
use super::lab::{self, Section};
use super::lab_common;
use crate::console::lab_api::{
    self, Dataset, DeviceMachine, DeviceRequest, Island, JobDocument, JobPlan, JobSummary, JobView,
    Recipe, SubmitError,
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
const BAR_WIDTH: f32 = 96.0;
const TRACK_HEIGHT: f32 = 6.0;
const PLAN_NOTE_SINGLE: &str =
    "One island on one machine: the job trains in a single process group.";
const PLAN_NOTE_TIGHT: &str =
    "The islands train as one group, synchronised every step over the leader’s rendezvous.";
const PLAN_NOTE_LOOSE: &str =
    "Each island trains on its own and the islands average through the hub between inner rounds.";

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
    if let (Some(lo), Some(hi)) = (min, max) {
        if hi < lo {
            return Err("Max devices is below min devices.".to_string());
        }
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

/// A budget bar: the share of the budget the runs have used, red once it is over.
fn budget_bar<'a>(used: f64, budget: f64) -> Element<'a, Msg> {
    let ratio = if budget > 0.0 {
        (used / budget).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let over = budget > 0.0 && used > budget;
    let fill = if over {
        crate::theme::palette().crit
    } else {
        crate::theme::palette().signal
    };
    let filled = (BAR_WIDTH * ratio as f32).max(0.0);
    container(
        row![
            container(
                Space::new()
                    .width(Length::Fixed(filled))
                    .height(Length::Fixed(TRACK_HEIGHT))
            )
            .style(move |_: &Theme| container::Style {
                background: Some(fill.into()),
                ..Default::default()
            }),
            Space::new()
                .width(Length::Fill)
                .height(Length::Fixed(TRACK_HEIGHT)),
        ]
        .width(Length::Fixed(BAR_WIDTH)),
    )
    .width(Length::Fixed(BAR_WIDTH))
    .height(Length::Fixed(TRACK_HEIGHT))
    .style(|_: &Theme| container::Style {
        border: Border {
            color: crate::theme::palette().rule,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .into()
}

/// `used / budget` in GPU-hours, with its bar.
fn used_cell<'a>(job: &JobSummary, alpha: f32) -> Element<'a, Msg> {
    let figure = format!(
        "{} / {}",
        lab_common::number(job.gpu_hours, 2),
        lab_common::number(job.budget_gpu_hours, 2)
    );
    column![
        ui::sans(figure, 12.0, tinted(crate::theme::palette().ink, alpha)),
        budget_bar(job.gpu_hours, job.budget_gpu_hours)
    ]
    .spacing(4)
    .align_x(Alignment::End)
    .into()
}

/// A text that stays on one line (the table's cells never wrap; the row's width is the share it gets).
fn single_line<'a>(content: text::Text<'a>) -> Element<'a, Msg> {
    content.wrapping(text::Wrapping::None).into()
}

/// One table cell: a fixed share of the row, its content right-aligned when `right`.
fn cell<'a>(portion: u16, right: bool, content: impl Into<Element<'a, Msg>>) -> Element<'a, Msg> {
    container(content.into())
        .clip(true)
        .width(Length::FillPortion(portion))
        .align_x(if right {
            Alignment::End
        } else {
            Alignment::Start
        })
        .into()
}

fn head_cell<'a>(label: &str, portion: u16, right: bool) -> Element<'a, Msg> {
    cell(
        portion,
        right,
        ui::sans(label.to_string(), 11.0, crate::theme::palette().ink_3),
    )
}

/// `text` cut to `max` characters, the last of them an ellipsis when it had to be cut.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let kept: String = text.chars().take(max - 1).collect();
    format!("{}…", kept.trim_end())
}

fn job_row<'a>(job: &JobSummary, alpha: f32, now: u64) -> Element<'a, Msg> {
    let id = job.id.clone();
    let content = row![
        cell(
            3,
            false,
            single_line(ui::mono(
                job.id.clone(),
                12.0,
                tinted(crate::theme::palette().ink, alpha)
            ))
        ),
        cell(
            3,
            false,
            ui::badge::<Msg>(&job.status, tone_of(&job.status))
        ),
        cell(
            2,
            false,
            single_line(ui::sans(
                job.mode.clone(),
                12.0,
                tinted(crate::theme::palette().ink_2, alpha)
            ))
        ),
        cell(
            4,
            false,
            single_line(ui::sans(
                job.recipe.clone(),
                12.0,
                tinted(crate::theme::palette().ink_2, alpha)
            ))
        ),
        cell(
            2,
            true,
            single_line(ui::sans(
                job.datasets.len().to_string(),
                12.0,
                tinted(crate::theme::palette().ink, alpha)
            ))
        ),
        cell(
            4,
            false,
            single_line(ui::sans(
                lab_common::time(job.created_ms),
                12.0,
                tinted(crate::theme::palette().ink_3, alpha)
            ))
        ),
        cell(
            2,
            true,
            single_line(ui::sans(
                lab_common::duration(duration_of(job, now)),
                12.0,
                tinted(crate::theme::palette().ink, alpha)
            ))
        ),
        cell(3, true, used_cell(job, alpha)),
        cell(
            4,
            false,
            single_line(ui::sans(
                clip(job.error.as_deref().unwrap_or("—"), 40),
                12.0,
                tinted(crate::theme::palette().crit, alpha)
            ))
        ),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .width(Length::Fill);
    button(
        container(content)
            .padding(Padding {
                top: 9.0,
                right: 0.0,
                bottom: 9.0,
                left: 0.0,
            })
            .width(Length::Fill),
    )
    .style(theme::register_row(false))
    .padding(0)
    .width(Length::Fill)
    .on_press(Msg::Go(Id::LabJobDetail(id)))
    .into()
}

/// A label and its value on one line of a card; the label fades with the card.
fn kv<'a>(label: &str, value: impl Into<Element<'a, Msg>>, alpha: f32) -> Element<'a, Msg> {
    let name = ui::sans(
        label.to_string(),
        10.5,
        tinted(crate::theme::palette().ink_3, alpha),
    );
    row![cell(1, false, name), cell(3, false, value)]
        .spacing(12)
        .align_y(Alignment::Center)
        .into()
}

/// One job's card. A card that arrived on a poll eases in: its text and border take `alpha`.
fn job_card<'a>(job: &JobSummary, now: u64, alpha: f32) -> Element<'a, Msg> {
    let body = column![
        row![
            ui::mono(
                job.id.clone(),
                13.0,
                tinted(crate::theme::palette().ink, alpha)
            ),
            Space::new().width(Length::Fill),
            ui::badge::<Msg>(&job.status, tone_of(&job.status))
        ]
        .align_y(Alignment::Center),
        kv(
            "Recipe",
            ui::sans(
                job.recipe.clone(),
                12.0,
                tinted(crate::theme::palette().ink_2, alpha)
            ),
            alpha
        ),
        kv(
            "Mode",
            ui::sans(
                job.mode.clone(),
                12.0,
                tinted(crate::theme::palette().ink_2, alpha)
            ),
            alpha
        ),
        kv(
            "Datasets",
            ui::sans(
                job.datasets.len().to_string(),
                12.0,
                tinted(crate::theme::palette().ink, alpha)
            ),
            alpha
        ),
        kv(
            "Created",
            ui::sans(
                lab_common::time(job.created_ms),
                12.0,
                tinted(crate::theme::palette().ink_3, alpha)
            ),
            alpha
        ),
        kv(
            "Duration",
            ui::sans(
                lab_common::duration(duration_of(job, now)),
                12.0,
                tinted(crate::theme::palette().ink, alpha)
            ),
            alpha
        ),
        kv("GPU-h used", used_cell(job, alpha), alpha),
        ui::sans(
            clip(job.error.as_deref().unwrap_or(""), 120),
            12.0,
            tinted(crate::theme::palette().crit, alpha)
        ),
    ]
    .spacing(8);
    let boxed = move |_: &Theme| container::Style {
        text_color: Some(tinted(crate::theme::palette().ink, alpha)),
        background: Some(tinted(crate::theme::palette().sheet, alpha).into()),
        border: Border {
            color: tinted(crate::theme::palette().rule_faint, alpha),
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    };
    button(container(body).padding(14).width(Length::Fill).style(boxed))
        .style(theme::register_row(false))
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

fn list_card<'a>(state: &'a State) -> Element<'a, Msg> {
    let all = &state.jobs;
    let labels: Vec<(Status, String)> = STATUSES
        .iter()
        .map(|(s, label)| {
            let n = if *s == Status::All {
                all.len()
            } else {
                all.iter().filter(|j| j.status == s.key()).count()
            };
            (*s, format!("{label} {n}"))
        })
        .collect();
    let status_refs: Vec<(Status, &str)> = labels.iter().map(|(s, l)| (*s, l.as_str())).collect();
    let status_strip = ui::seg(&status_refs, state.status, Msg::Filter);
    let view_strip = ui::seg(
        &[(View::Table, "Table"), (View::Cards, "Cards")],
        state.view,
        Msg::View,
    );

    let mut recipe_names: Vec<String> = all.iter().map(|j| j.recipe.clone()).collect();
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
    let recipe_pick =
        pick_list(recipe_options, recipe_selected, Msg::RecipeFilter).width(Length::Fixed(240.0));

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
    let mode_pick =
        pick_list(mode_options, mode_selected, Msg::ModeFilter).width(Length::Fixed(200.0));

    let search = text_input("Search by job id", &state.query)
        .on_input(Msg::Query)
        .padding([8, 10])
        .size(13.0)
        .width(Length::Fill)
        .style(theme::input);

    let rows = visible(state);
    let now = lab_common::now_ms();
    let fresh_at = &state.fresh;
    let body: Element<'a, Msg> = if !state.loaded {
        ui::loading()
    } else if all.is_empty() {
        column![
            ui::empty("play", "No jobs yet", "Pick a vetted recipe and a dataset, set a budget, and preview the plan before you submit."),
            container(ui::btn("New job", Some("plus"), ui::Kind::Primary, Some(Msg::ScrollToForm))).center_x(Length::Fill),
        ]
        .spacing(14)
        .into()
    } else if rows.is_empty() {
        column![
            ui::empty(
                "search",
                "No jobs match",
                "Clear the filters to see every job."
            ),
            container(ui::btn(
                "Clear filters",
                None,
                ui::Kind::Secondary,
                Some(Msg::ClearFilters)
            ))
            .center_x(Length::Fill),
        ]
        .spacing(14)
        .into()
    } else if state.view == View::Table {
        let header = row![
            head_cell("ID", 3, false),
            head_cell("Status", 2, false),
            head_cell("Mode", 2, false),
            head_cell("Recipe", 4, false),
            head_cell("Datasets", 2, true),
            head_cell("Created", 3, false),
            head_cell("Duration", 2, true),
            head_cell("GPU-h used", 3, true),
            head_cell("Error", 4, false),
        ]
        .spacing(12)
        .width(Length::Fill);
        let lines: Vec<Element<'a, Msg>> = rows
            .iter()
            .map(|j| job_row(j, fade(fresh_at, &j.id), now))
            .collect();
        column![header, column(lines).spacing(0)].spacing(4).into()
    } else {
        let cards: Vec<Element<'a, Msg>> = rows
            .chunks(2)
            .map(|pair| {
                let cards: Vec<Element<'a, Msg>> = pair
                    .iter()
                    .map(|j| {
                        container(job_card(j, now, fade(fresh_at, &j.id)))
                            .width(Length::FillPortion(1))
                            .into()
                    })
                    .collect();
                row(cards).spacing(16).into()
            })
            .collect();
        column(cards).spacing(16).into()
    };

    let count_line = ui::faint(format!("{} of {} shown", rows.len(), all.len()));
    let errors: Option<Element<'a, Msg>> = state.error.as_deref().map(ui::error_line);
    let toolbar = column![
        row![status_strip, Space::new().width(Length::Fill), view_strip]
            .spacing(12)
            .align_y(Alignment::Center),
        row![recipe_pick, mode_pick, search]
            .spacing(12)
            .align_y(Alignment::Center),
    ]
    .spacing(12);
    ui::card(
        "Job list",
        column![
            count_line,
            toolbar,
            errors.unwrap_or_else(|| Space::new().into()),
            body
        ]
        .spacing(14)
        .into(),
    )
}

/// One field of the form: its name in mono caps, the control, and a faint hint.
fn field<'a>(title: &str, control: Element<'a, Msg>, hint: &str) -> Element<'a, Msg> {
    column![ui::label(title), control, ui::faint(hint.to_string())]
        .spacing(6)
        .into()
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

fn unvetted_note(state: &State) -> String {
    let vetted = recipe_options(state).len();
    let unvetted: Vec<&str> = state
        .recipes
        .iter()
        .filter(|r| r.vetted_by.is_none() || r.vetted_ms.is_none())
        .map(|r| r.id.as_str())
        .collect();
    if vetted == 0 {
        "No vetted recipe yet. The owner vets one on the Recipes tab.".to_string()
    } else if unvetted.is_empty() {
        String::new()
    } else {
        format!("Not vetted, so not selectable: {}.", unvetted.join(", "))
    }
}

fn dataset_rows<'a>(state: &'a State) -> Element<'a, Msg> {
    if state.datasets.is_empty() {
        return ui::faint("No datasets yet. Make one on the Datasets tab.").into();
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
                ui::mono(d.id.clone(), 11.0, crate::theme::palette().ink_3),
                Space::new().width(Length::Fill),
                ui::faint(format!("{} · {} rows", d.kind, lab_common::count(d.rows))),
            ]
            .spacing(12)
            .align_y(Alignment::Center);
            if d.frozen_rows > 0 {
                line = line.push(ui::badge::<Msg>(
                    &format!("frozen {}", lab_common::count(d.frozen_rows)),
                    ui::Tone::Signal,
                ));
            }
            container(line).padding([8, 12]).width(Length::Fill).into()
        })
        .collect();
    scroll_box(column(rows).spacing(0).into())
}

fn machine_rows<'a>(state: &'a State) -> Element<'a, Msg> {
    if state.machines.is_empty() {
        return ui::faint("No machines report devices.").into();
    }
    let rows: Vec<Element<'a, Msg>> = state
        .machines
        .iter()
        .map(|m| {
            let id = m.id.clone();
            let picked = state.form.machines.contains(&m.id);
            let devices = m.devices.len();
            let line = row![
                checkbox(picked)
                    .label(m.id.clone())
                    .on_toggle(move |on| Msg::ToggleMachine(id.clone(), on)),
                Space::new().width(Length::Fill),
                ui::faint(format!(
                    "{devices} device{}",
                    if devices == 1 { "" } else { "s" }
                )),
                ui::badge::<Msg>(
                    if m.online { "online" } else { "offline" },
                    if m.online {
                        ui::Tone::Ok
                    } else {
                        ui::Tone::Muted
                    }
                ),
            ]
            .spacing(12)
            .align_y(Alignment::Center);
            container(line).padding([8, 12]).width(Length::Fill).into()
        })
        .collect();
    scroll_box(column(rows).spacing(0).into())
}

/// A bordered box that scrolls past 240 px.
fn scroll_box<'a>(body: Element<'a, Msg>) -> Element<'a, Msg> {
    container(ui::scroll(body).width(Length::Fill))
        .max_height(240.0)
        .style(theme::boxed)
        .width(Length::Fill)
        .into()
}

fn device_pin<'a>(state: &'a State) -> Element<'a, Msg> {
    let f = &state.form;
    let min = text_input("e.g. 1", &f.min_devices)
        .on_input(Msg::MinDevices)
        .padding([8, 10])
        .size(13.0)
        .width(Length::Fill)
        .style(theme::input);
    let max = text_input("e.g. 4", &f.max_devices)
        .on_input(Msg::MaxDevices)
        .padding([8, 10])
        .size(13.0)
        .width(Length::Fill)
        .style(theme::input);
    let mem = text_input("e.g. 16384", &f.min_memory)
        .on_input(Msg::MinMemory)
        .padding([8, 10])
        .size(13.0)
        .width(Length::Fill)
        .style(theme::input);
    let counts = row![
        field(
            "Min devices",
            min.into(),
            "Optional. Whole number, 1 or more."
        ),
        field("Max devices", max.into(), "Optional. Defaults to the min."),
        field(
            "Min memory (MiB)",
            mem.into(),
            "Optional. Each device must have at least this much."
        ),
    ]
    .spacing(16)
    .width(Length::Fill);
    let backends = row(BACKENDS
        .iter()
        .map(|b| {
            let name: &'static str = b;
            checkbox(f.backends.contains(*b))
                .label(*b)
                .on_toggle(move |on| Msg::ToggleBackend(name, on))
                .into()
        })
        .collect::<Vec<Element<'a, Msg>>>())
    .spacing(16)
    .wrap();
    let mode_options: Vec<Opt<Option<String>>> = MODE_LABELS
        .iter()
        .map(|(label, value)| Opt {
            value: value.map(|v| v.to_string()),
            label: (*label).to_string(),
        })
        .collect();
    let mode_selected = mode_options.iter().find(|o| o.value == f.mode).cloned();
    let mode = pick_list(mode_options, mode_selected, Msg::PickMode).width(Length::Fill);
    let allow = checkbox(f.allow_cpu)
        .label("Allow CPU devices")
        .on_toggle(Msg::AllowCpu);
    container(
        column![
            ui::label("Device pin (optional)"),
            counts,
            field(
                "Backends",
                backends.into(),
                "None picked means any backend the planner allows."
            ),
            field(
                "Mode",
                mode.into(),
                "Auto lets the planner choose between single, tight and loose."
            ),
            field(
                "Machines",
                machine_rows(state),
                "None picked means any machine."
            ),
            allow,
        ]
        .spacing(16),
    )
    .padding(14)
    .width(Length::Fill)
    .style(theme::boxed)
    .into()
}

/// A plan as a diagram: the facts, then one box per island with its leader and machines' device indexes.
pub fn plan_view<'a>(plan: &JobPlan) -> Element<'a, Msg> {
    let fact = |label: &str, value: String| -> Element<'a, Msg> {
        container(
            column![
                ui::label(label),
                ui::sans(value, 13.0, crate::theme::palette().ink).wrapping(text::Wrapping::Glyph)
            ]
            .spacing(4),
        )
        .padding([10, 12])
        .width(Length::FillPortion(1))
        .style(theme::boxed)
        .into()
    };
    let mut facts = vec![
        fact("Mode", plan.mode.clone()),
        fact("Devices", lab_common::count(plan.devices_total)),
        fact("Inner steps", lab_common::count(plan.inner_steps)),
        fact("Staleness", lab_common::count(plan.staleness)),
    ];
    if let Some(rounds) = plan.rounds {
        facts.push(fact("Rounds", lab_common::count(rounds)));
    }
    facts.push(fact(
        "Frozen",
        plan.frozen.clone().unwrap_or_else(|| "none".to_string()),
    ));
    let note = match plan.mode.as_str() {
        "single" => Some(PLAN_NOTE_SINGLE),
        "tight" => Some(PLAN_NOTE_TIGHT),
        "loose" => Some(PLAN_NOTE_LOOSE),
        _ => None,
    };
    let islands: Vec<Element<'a, Msg>> = plan.islands.iter().map(island_box).collect();
    let note_line: Element<'a, Msg> = match note {
        Some(n) => ui::faint(n.to_string()).into(),
        None => Space::new().into(),
    };
    let island_row: Element<'a, Msg> = if islands.is_empty() {
        ui::faint("The plan has no islands.").into()
    } else {
        row(islands).spacing(12).wrap().into()
    };
    column![
        row(facts).spacing(10).width(Length::Fill),
        note_line,
        island_row,
        row![
            ui::label("Output"),
            ui::mono(plan.output_dir.clone(), 12.0, crate::theme::palette().ink_2)
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    ]
    .spacing(14)
    .into()
}

/// A plan's text field, or a dash when the plan leaves it empty (as the web shows it).
fn dash_if_empty(value: &str) -> &str {
    if value.is_empty() { "—" } else { value }
}

fn island_box<'a>(island: &Island) -> Element<'a, Msg> {
    let machines: Vec<Element<'a, Msg>> = island
        .machines
        .iter()
        .map(|m| {
            let devices: Vec<Element<'a, Msg>> = m
                .devices
                .iter()
                .map(|d| {
                    container(ui::mono(
                        format!("device {d}"),
                        11.0,
                        crate::theme::palette().ink_2,
                    ))
                    .padding([2, 6])
                    .style(chip)
                    .into()
                })
                .collect();
            column![
                ui::mono(m.id.clone(), 12.0, crate::theme::palette().ink),
                row(devices).spacing(4).wrap()
            ]
            .spacing(6)
            .into()
        })
        .collect();
    container(
        column![
            row![
                ui::mono(island.id.clone(), 13.0, crate::theme::palette().ink),
                Space::new().width(Length::Fill),
                ui::badge::<Msg>(&island.backend, ui::Tone::Signal)
            ]
            .align_y(Alignment::Center),
            ui::faint(format!("Leader {}", dash_if_empty(&island.leader))),
            ui::faint(format!("Rendezvous {}", dash_if_empty(&island.rdzv))),
            column(machines).spacing(10),
        ]
        .spacing(10),
    )
    .padding(12)
    .width(Length::FillPortion(1))
    .style(theme::boxed)
    .into()
}

fn chip(_: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: crate::theme::palette().rule_faint,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
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
        .placeholder("Choose a vetted recipe")
        .width(Length::Fill);

    let search = text_editor(&f.search)
        .placeholder("{\"lr\": [0.001, 0.01]}  or one key: value per line")
        .font(SANS)
        .size(13.0)
        .height(Length::Fixed(120.0))
        .on_action(Msg::Search);
    let budget = text_input("e.g. 1.5", &f.budget)
        .on_input(Msg::Budget)
        .padding([8, 10])
        .size(13.0)
        .width(Length::Fill)
        .style(theme::input);
    let train = text_editor(&f.train)
        .placeholder("{\"steps\": 500}")
        .font(SANS)
        .size(13.0)
        .height(Length::Fixed(90.0))
        .on_action(Msg::Train);

    let errors = build(f).err().unwrap_or_default();
    let doc_ok = errors.is_empty();
    let fresh_now = fresh(state);
    let busy = state.previewing || state.submitting;
    let problem_list: Option<Element<'a, Msg>> = (state.attempted && !doc_ok).then(|| {
        column(
            errors
                .iter()
                .map(|e| ui::sans(e.clone(), 12.0, crate::theme::palette().warn).into())
                .collect::<Vec<Element<'a, Msg>>>(),
        )
        .spacing(4)
        .into()
    });

    let hint: Element<'a, Msg> = if fresh_now {
        Space::new().into()
    } else {
        ui::faint("Preview the plan for these inputs first.").into()
    };
    let actions = row![
        ui::btn(
            "Preview plan",
            None,
            ui::Kind::Primary,
            (!busy).then_some(Msg::Preview)
        ),
        ui::btn(
            "Submit",
            None,
            ui::Kind::Secondary,
            (fresh_now && !busy).then_some(Msg::AskSubmit)
        ),
        hint,
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    let options_note: Element<'a, Msg> = match &state.options_error {
        Some(message) => ui::error_line(message),
        None => Space::new().into(),
    };

    let preview = preview_area(state, &errors, fresh_now);
    let confirm = confirm_area(state, fresh_now, doc_ok);
    let submit_note = submit_area(state);

    let body = column![
        options_note,
        field("Recipe", recipe.into(), &unvetted_note(state)),
        field(
            "Datasets",
            dataset_rows(state),
            "Pick one or more. A frozen count is the rows the dataset has frozen."
        ),
        field(
            "Search space",
            search.into(),
            "Optional. A JSON object, or one key: value per line."
        ),
        field(
            "Budget (GPU-hours)",
            budget.into(),
            "The job stops once it has used this much GPU time."
        ),
        device_pin(state),
        field(
            "Train overrides",
            train.into(),
            "Optional. A JSON object, such as {\"steps\": 500}. Empty keeps the recipe defaults."
        ),
        problem_list.unwrap_or_else(|| Space::new().into()),
        actions,
        preview,
        confirm,
        submit_note,
    ]
    .spacing(18);
    ui::card("New job", body.into())
}

fn preview_area<'a>(state: &'a State, errors: &[String], fresh_now: bool) -> Element<'a, Msg> {
    let key = current_key(&state.form);
    if state.previewing {
        return ui::faint("Planning the job…").into();
    }
    if let (Some((k, message)), Some(now)) = (&state.preview_error, &key) {
        if k == now {
            return ui::error_line(message);
        }
    }
    if let Some(p) = &state.preview {
        if fresh_now {
            return plan_view(&p.plan);
        }
        return ui::faint(
            "The inputs changed since the last preview. Preview again before you submit.",
        )
        .into();
    }
    if !errors.is_empty() {
        return ui::faint(
            "Fill in the form, then preview the plan. The plan shows where the job would run.",
        )
        .into();
    }
    ui::faint("Preview the plan to see where the job would run. Submit unlocks after a preview of these inputs.").into()
}

fn confirm_area<'a>(state: &'a State, fresh_now: bool, doc_ok: bool) -> Element<'a, Msg> {
    if !(state.confirming && fresh_now && doc_ok) {
        return Space::new().into();
    }
    let Ok(doc) = build(&state.form) else {
        return Space::new().into();
    };
    let n = doc.datasets.len();
    let summary = format!(
        "Submit {} on {n} dataset{} with a budget of {} GPU-hours? The job dispatches to the owner's GPU machines now.",
        doc.recipe,
        if n == 1 { "" } else { "s" },
        doc.budget_gpu_hours
    );
    container(
        column![
            ui::faint(summary),
            row![
                ui::btn(
                    "Confirm submit",
                    None,
                    ui::Kind::Primary,
                    (!state.submitting).then_some(Msg::ConfirmSubmit)
                ),
                ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CancelConfirm)),
            ]
            .spacing(12),
        ]
        .spacing(12),
    )
    .padding(14)
    .width(Length::Fill)
    .style(theme::boxed)
    .into()
}

fn submit_area<'a>(state: &'a State) -> Element<'a, Msg> {
    if state.submitting {
        return ui::faint("Submitting…").into();
    }
    match &state.submit_error {
        None => Space::new().into(),
        Some((message, Some(job))) => column![
            ui::error_line(&format!("The job was recorded as failed: {message}")),
            ui::link("Open the job", Msg::Go(Id::LabJobDetail(job.clone())), true),
        ]
        .spacing(6)
        .into(),
        Some((message, None)) => ui::error_line(message),
    }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let head = ui::view_head(
        "Jobs",
        vec![ui::btn(
            "New job",
            Some("plus"),
            ui::Kind::Primary,
            Some(Msg::ScrollToForm),
        )],
    );
    let body = column![head, list_card(state), form_card(state)]
        .spacing(22)
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
