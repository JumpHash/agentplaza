//! One job (`LabJobDetail(id)`): the header, the status timeline, the plan (islands, machines,
//! devices), live charts (loss and learning rate per island, eval metrics, loose-mode rounds,
//! trials), per-device utilization, a log tail, the budget, outputs and metrics, and the error. The
//! web's `views/lab-job-detail.ts` shows the same, and both draw the same numbers: the job's series
//! plus the live lines the view has not absorbed yet (a line whose seq is above the view's seq). The
//! view reloads on each `training` frame for this job (at most once a second; the last one trails)
//! and on the screen's tick.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use iced::widget::operation::snap_to;
use iced::widget::{Id as WidgetId, Space, column, container, row, scrollable, text};
use iced::{Alignment, Border, Color, Element, Length, Padding, Task, Theme};
use serde_json::Value;

use super::Id;
use super::lab::{self, Section};
use super::lab_common::{self, TrainingFrame};
use crate::console::chart;
use crate::console::client::{self, Client};
use crate::console::lab_api::{
    self, JobEvent, JobEventsPage, JobPlan, JobRun, JobView, SeriesPoint,
};
use crate::console::{Ctx, ui};
use crate::theme::SANS_SEMIBOLD;

/// Live lines kept for the screen: the newest this many (the charts read them as they arrive).
const LIVE_KEPT: usize = 2_000;
/// Events kept for the log tail: the newest this many.
const LOG_KEPT: usize = 300;
/// A log line's longest length, in characters; longer lines end in an ellipsis.
const LINE_MAX: usize = 240;
/// At most one load a second from the stream; the last request waits for the gap.
const THROTTLE: Duration = Duration::from_secs(1);
/// The log tail's scroll id.
const LOG_ID: &str = "lab-job-log";

/// The job view as this screen holds it: the view, the events after the last one kept, and the
/// request state. `live` is the hub's `training` frames for this job since the screen opened.
#[derive(Debug, Default)]
pub struct State {
    /// The job this screen shows.
    pub job: String,
    /// The job's live `training` frames since the screen opened, oldest first (the hub's stream,
    /// filtered to `job` in `update`). The charts draw from these as well as the job's series.
    pub live: Vec<TrainingFrame>,
    /// When each of `live` arrived (unix ms), in step with it: a live point's time when the view has not absorbed it.
    pub live_at: Vec<u64>,
    /// The last job view that loaded.
    pub view: Option<JobView>,
    /// The job is not known to the hub.
    pub not_found: bool,
    /// The last load's error, while no view has come since.
    pub error: Option<String>,
    /// The cancel's error, until the next cancel.
    pub cancel_error: Option<String>,
    /// The log tail: the newest `LOG_KEPT` events.
    pub logs: Vec<JobEvent>,
    /// The sequence number the next events request starts after.
    pub logs_next: u64,
    /// A load is in flight.
    pub loading: bool,
    /// A load was asked for while one was in flight: the next one runs when it lands.
    pub dirty: bool,
    /// A trailing load is queued for the throttle's gap.
    pub flush_queued: bool,
    /// When the last load started.
    pub last_load: Option<Instant>,
    /// The cancel confirmation is open.
    pub confirming: bool,
    /// A cancel request is in flight.
    pub cancelling: bool,
    /// The log tail is scrolled up from its end (a new line does not pull it down).
    pub log_up: bool,
}

/// What one load brings: the job view, and the events after the last one kept (if they loaded).
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub view: JobView,
    events: Option<JobEventsPage>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    /// A `training` frame from the hub's stream (every job's; `update` keeps this screen's).
    Live(TrainingFrame),
    /// A load landed.
    Loaded(Result<Snapshot, String>),
    /// The throttle's gap has passed: load now.
    Flush,
    /// The log tail scrolled to this relative offset.
    LogScroll(f32),
    /// Open the cancel confirmation.
    AskCancel,
    /// Close the cancel confirmation.
    KeepRunning,
    /// Cancel the job.
    ConfirmCancel,
    /// The cancel landed.
    Cancelled(Result<JobView, String>),
}

/// The navigation a message asks for.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    start_load(state, ctx)
}

fn start_load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    if state.job.is_empty() {
        return Task::none();
    }
    if state.loading {
        state.dirty = true;
        return Task::none();
    }
    state.loading = true;
    state.last_load = Some(Instant::now());
    let c = ctx.client.clone();
    let id = state.job.clone();
    let after = state.logs_next;
    client::task(move || fetch(&c, &id, after), Msg::Loaded)
}

/// A load that waits out the throttle's gap: at most one load a second; the last request trails.
fn throttled(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let elapsed = match state.last_load {
        Some(at) => at.elapsed(),
        None => return start_load(state, ctx),
    };
    if elapsed >= THROTTLE {
        return start_load(state, ctx);
    }
    if state.flush_queued {
        return Task::none();
    }
    state.flush_queued = true;
    let wait = THROTTLE - elapsed;
    Task::perform(async move { std::thread::sleep(wait) }, |()| Msg::Flush)
}

fn fetch(c: &Client, id: &str, after: u64) -> Result<Snapshot, String> {
    let view = lab_api::get_job(c, id)?;
    let events = lab_api::job_events(c, id, after).ok();
    Ok(Snapshot { view, events })
}

/// The hub's answer for an unknown job: its `error` text is "no such job".
fn is_not_found(err: &str) -> bool {
    err == "no such job" || err == "HTTP 404"
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Live(frame) => {
            if frame.job != state.job {
                return Task::none();
            }
            state.live.push(frame);
            state.live_at.push(lab_common::now_ms());
            let excess = state.live.len().saturating_sub(LIVE_KEPT);
            state.live.drain(..excess);
            let excess_at = state.live_at.len().saturating_sub(LIVE_KEPT);
            state.live_at.drain(..excess_at);
            throttled(state, ctx)
        }
        Msg::Loaded(result) => {
            state.loading = false;
            match result {
                Ok(snap) => {
                    state.not_found = false;
                    state.error = None;
                    if let Some(page) = snap.events {
                        state.logs.extend(page.events);
                        let excess = state.logs.len().saturating_sub(LOG_KEPT);
                        state.logs.drain(..excess);
                        state.logs_next = state.logs_next.max(page.next);
                    }
                    state.view = Some(snap.view);
                }
                Err(err) if is_not_found(&err) => {
                    state.not_found = true;
                    state.view = None;
                    state.error = None;
                }
                Err(err) => state.error = Some(err),
            }
            let follow = if state.log_up {
                Task::none()
            } else {
                snap_to(WidgetId::new(LOG_ID), scrollable::RelativeOffset::END)
            };
            let again = if state.dirty {
                state.dirty = false;
                throttled(state, ctx)
            } else {
                Task::none()
            };
            Task::batch([follow, again])
        }
        Msg::Flush => {
            state.flush_queued = false;
            start_load(state, ctx)
        }
        Msg::LogScroll(y) => {
            state.log_up = !(y.is_nan() || y >= 0.999);
            Task::none()
        }
        Msg::AskCancel => {
            state.confirming = true;
            Task::none()
        }
        Msg::KeepRunning => {
            state.confirming = false;
            Task::none()
        }
        Msg::ConfirmCancel => {
            state.confirming = false;
            state.cancelling = true;
            state.cancel_error = None;
            let c = ctx.client.clone();
            let id = state.job.clone();
            client::task(move || lab_api::cancel_job(&c, &id), Msg::Cancelled)
        }
        Msg::Cancelled(Ok(_)) => {
            state.cancelling = false;
            start_load(state, ctx)
        }
        Msg::Cancelled(Err(err)) => {
            state.cancelling = false;
            state.cancel_error = Some(err);
            Task::none()
        }
    }
}

// ---- The job's points: the series, then the live lines the view has not absorbed --------------

/// One trainer event as the job's series keeps it: the same mapping the apid's `absorb` makes.
fn point_of(island: &str, event: &Value, t: u64) -> Option<SeriesPoint> {
    let uint = |k: &str| event.get(k).and_then(Value::as_u64);
    let num = |k: &str| event.get(k).and_then(Value::as_f64);
    let base = |kind: &str| SeriesPoint {
        kind: kind.to_string(),
        island: island.to_string(),
        t,
        ..Default::default()
    };
    match event.get("ev")?.as_str()? {
        "step" => Some(SeriesPoint {
            step: uint("step"),
            loss: num("loss"),
            lr: num("lr"),
            samples: uint("samples"),
            ..base("step")
        }),
        "eval" => Some(SeriesPoint {
            step: uint("step"),
            split: event
                .get("split")
                .and_then(Value::as_str)
                .map(str::to_string),
            metrics: event.get("metrics").filter(|m| m.is_object()).cloned(),
            ..base("eval")
        }),
        "round" => Some(SeriesPoint {
            round: uint("round"),
            accepted: event.get("accepted").and_then(Value::as_bool),
            hub_round: uint("hub_round"),
            step: uint("step"),
            ..base("round")
        }),
        "trial" => Some(SeriesPoint {
            trial: uint("trial"),
            objective: num("objective"),
            ..base("trial")
        }),
        _ => None,
    }
}

/// The job's series, then the live lines the view has not absorbed (seq above the view's seq), in seq order.
fn points_of(view: &JobView, live: &[TrainingFrame], live_at: &[u64]) -> Vec<SeriesPoint> {
    let mut fresh: BTreeMap<u64, (String, Value, u64)> = BTreeMap::new();
    for (frame, at) in live.iter().zip(live_at) {
        let seq = frame.line["seq"].as_u64().unwrap_or(0);
        if seq > view.seq {
            let island = frame.line["island"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            fresh
                .entry(seq)
                .or_insert_with(|| (island, frame.line["event"].clone(), *at));
        }
    }
    let mut points = view.series.clone();
    points.extend(
        fresh
            .values()
            .filter_map(|(island, event, at)| point_of(island, event, *at)),
    );
    points
}

/// One chart: its series, the x labels and the tooltip heads (one each per point), the short name the
/// tooltip rows use, the axis format and its summary line.
struct Spec {
    short: String,
    labels: Vec<String>,
    heads: Vec<String>,
    values: Vec<f64>,
    format: fn(f64) -> String,
    summary: String,
    color: Color,
}

fn fix4(v: f64) -> String {
    format!("{v:.4}")
}

fn exp2(v: f64) -> String {
    format!("{v:.2e}")
}

fn loss_color() -> Color {
    crate::theme::palette().info
}
fn lr_color() -> Color {
    crate::theme::palette().ok
}
/// The web's chart palette, in the current mode.
fn palette() -> [Color; 6] {
    let p = crate::theme::palette();
    [p.warn, p.chart_4, p.chart_8, p.chart_1, p.ink_2, p.crit]
}

fn step_text(p: &SeriesPoint) -> String {
    p.step.map_or_else(|| "?".to_string(), |s| s.to_string())
}

/// A time of day, local, `HH:MM:SS`; a dash for no time (the web's `clock`).
fn clock(ms: u64) -> String {
    if ms == 0 {
        return "—".to_string();
    }
    match chrono::DateTime::from_timestamp_millis(ms as i64) {
        Some(utc) => utc
            .with_timezone(&chrono::Local)
            .format("%H:%M:%S")
            .to_string(),
        None => "—".to_string(),
    }
}

/// Adds one chart over `pts` (already filtered to the ones that have a value), if it has any.
#[allow(clippy::too_many_arguments)]
fn push_chart(
    out: &mut Vec<Spec>,
    pts: &[&SeriesPoint],
    pick: impl Fn(&SeriesPoint) -> Option<f64>,
    title: String,
    short: &str,
    format: fn(f64) -> String,
    color: Color,
    best: bool,
) {
    let Some(last) = pts.last() else { return };
    let values: Vec<f64> = pts.iter().filter_map(|p| pick(p)).collect();
    let Some(&last_value) = values.last() else {
        return;
    };
    let mut parts = vec![format!(
        "last {} at step {}",
        format(last_value),
        step_text(last)
    )];
    if best {
        let min = values.iter().copied().fold(f64::INFINITY, f64::min);
        parts.push(format!("best {}", format(min)));
    }
    if values.len() < 2 {
        parts.push("one point so far".to_string());
    }
    let labels = pts.iter().map(|p| step_text(p)).collect();
    let heads = pts
        .iter()
        .map(|p| format!("step {} · {}", step_text(p), clock(p.t)))
        .collect();
    let summary = format!("{title} · {}", parts.join(" · "));
    out.push(Spec {
        short: short.to_string(),
        labels,
        heads,
        values,
        format,
        summary,
        color,
    });
}

/// Loss and learning rate per island, then one chart per eval metric (sorted by name).
fn chart_specs(points: &[SeriesPoint]) -> Vec<Spec> {
    let mut out = Vec::new();
    let steps: Vec<&SeriesPoint> = points.iter().filter(|p| p.kind == "step").collect();
    let mut islands: Vec<&str> = Vec::new();
    for p in &steps {
        if !islands.contains(&p.island.as_str()) {
            islands.push(p.island.as_str());
        }
    }
    for isl in islands {
        let loss: Vec<&SeriesPoint> = steps
            .iter()
            .copied()
            .filter(|p| p.island == isl && p.loss.is_some())
            .collect();
        push_chart(
            &mut out,
            &loss,
            |p| p.loss,
            format!("Loss · island {isl}"),
            "loss",
            fix4,
            loss_color(),
            true,
        );
        let lr: Vec<&SeriesPoint> = steps
            .iter()
            .copied()
            .filter(|p| p.island == isl && p.lr.is_some())
            .collect();
        push_chart(
            &mut out,
            &lr,
            |p| p.lr,
            format!("Learning rate · island {isl}"),
            "lr",
            exp2,
            lr_color(),
            false,
        );
    }
    let evals: Vec<&SeriesPoint> = points
        .iter()
        .filter(|p| p.kind == "eval" && p.metrics.is_some())
        .collect();
    let names: BTreeSet<&str> = evals
        .iter()
        .flat_map(|p| {
            p.metrics
                .iter()
                .flat_map(|m| m.as_object())
                .flat_map(|o| o.keys().map(String::as_str))
        })
        .collect();
    for (i, name) in names.into_iter().enumerate() {
        let pts: Vec<&SeriesPoint> = evals
            .iter()
            .copied()
            .filter(|p| metric(p, name).is_some())
            .collect();
        let color = palette()[i % palette().len()];
        push_chart(
            &mut out,
            &pts,
            |p| metric(p, name),
            format!("Eval · {name}"),
            name,
            fix4,
            color,
            false,
        );
    }
    out
}

fn metric(p: &SeriesPoint, name: &str) -> Option<f64> {
    p.metrics.as_ref()?.get(name)?.as_f64()
}

// ---- Text the screen shares with the web -----------------------------------------------------

/// A number's text: a whole number without a fraction, otherwise its shortest form.
fn number_text(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{}", f as i64)
    } else {
        format!("{f}")
    }
}

/// A JSON value with its object keys sorted (the web's `canon`).
fn canon(v: &Value) -> String {
    match v {
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(canon).collect::<Vec<_>>().join(",")
        ),
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let parts: Vec<String> = keys
                .iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        Value::String((*k).clone()),
                        canon(&map[k.as_str()])
                    )
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        Value::Number(n) => number_text(n.as_f64().unwrap_or(f64::NAN)),
        other => other.to_string(),
    }
}

/// One log line: the event's name, then its other fields by name.
fn log_text(event: &Value) -> String {
    let mut parts = vec![
        event
            .get("ev")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    ];
    let mut keys: Vec<&String> = event
        .as_object()
        .map(|m| m.keys().filter(|k| k.as_str() != "ev").collect())
        .unwrap_or_default();
    keys.sort();
    for k in keys {
        let shown = match &event[k.as_str()] {
            Value::String(s) => s.clone(),
            other => canon(other),
        };
        parts.push(format!("{k}={shown}"));
    }
    let line = parts.join(" ");
    if line.chars().count() > LINE_MAX {
        format!("{}…", line.chars().take(LINE_MAX - 1).collect::<String>())
    } else {
        line
    }
}

/// A metric's value: a whole number as is, a fraction to 4 places, a string as it is.
fn metric_text(v: &Value) -> String {
    match v {
        Value::Number(n) => {
            let f = n.as_f64().unwrap_or(f64::NAN);
            if f.fract() == 0.0 {
                number_text(f)
            } else {
                fix4(f)
            }
        }
        Value::String(s) => s.clone(),
        other => canon(other),
    }
}

/// Two decimals, no grouping (the web's `toFixed(2)`); a dash for a missing value.
fn dash2(v: f64) -> String {
    if v.is_finite() {
        format!("{v:.2}")
    } else {
        "—".to_string()
    }
}

fn tone_of(status: &str) -> ui::Tone {
    match status {
        "running" => ui::Tone::Info,
        "done" => ui::Tone::Ok,
        "failed" => ui::Tone::Danger,
        "cancelled" => ui::Tone::Warn,
        _ => ui::Tone::Muted,
    }
}

// ---- Timeline --------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum StageState {
    Done,
    Current,
    Pending,
    Skipped,
}

struct Stage {
    label: &'static str,
    at: Option<u64>,
    sub: String,
    state: StageState,
}

/// Three stages: queued, running (from the first run's start), and the end (the status, its time and any error).
fn stages_of(v: &JobView) -> Vec<Stage> {
    let terminal = matches!(v.status.as_str(), "done" | "failed" | "cancelled");
    let starts: Vec<u64> = v
        .runs
        .iter()
        .map(|r| r.started_ms)
        .filter(|t| *t > 0)
        .collect();
    let had_runs = !v.runs.is_empty();
    let queued = v.status == "queued";
    let running = v.status == "running";
    let run_sub = if had_runs {
        format!(
            "{} run{}",
            v.runs.len(),
            if v.runs.len() == 1 { "" } else { "s" }
        )
    } else if terminal {
        "Never started.".to_string()
    } else {
        "Not started yet.".to_string()
    };
    let end_sub = if v.status == "failed" && v.error.is_some() {
        v.error.clone().unwrap_or_default()
    } else if terminal {
        String::new()
    } else {
        "Not finished.".to_string()
    };
    let end_label = match v.status.as_str() {
        "done" => "Done",
        "failed" => "Failed",
        "cancelled" => "Cancelled",
        _ => "Finished",
    };
    let run_state = if running {
        StageState::Current
    } else if terminal && had_runs {
        StageState::Done
    } else if terminal {
        StageState::Skipped
    } else {
        StageState::Pending
    };
    vec![
        Stage {
            label: "Queued",
            at: Some(v.created_ms),
            sub: if queued {
                "Waits for a free device.".to_string()
            } else {
                String::new()
            },
            state: if queued {
                StageState::Current
            } else {
                StageState::Done
            },
        },
        Stage {
            label: "Running",
            at: starts.iter().min().copied(),
            sub: run_sub,
            state: run_state,
        },
        Stage {
            label: end_label,
            at: terminal.then_some(v.updated_ms),
            sub: end_sub,
            state: if terminal {
                StageState::Current
            } else {
                StageState::Pending
            },
        },
    ]
}

// ---- Shared pieces ---------------------------------------------------------------------------

fn txt<'a>(content: impl Into<String>) -> Element<'a, Msg> {
    ui::sans(content.into(), 12.5, crate::theme::palette().ink).into()
}

/// One figure of the job's stats, as the web's `.stat`: a raised card with its label over its value.
fn stat<'a>(label: &str, value: String) -> Element<'a, Msg> {
    container(
        column![
            ui::label(label),
            text(value)
                .font(SANS_SEMIBOLD)
                .size(15.0)
                .color(crate::theme::palette().ink)
        ]
        .spacing(6),
    )
    .padding([12, 16])
    .width(Length::Fill)
    .style(ui::card_style)
    .into()
}

/// A table as the web's `.tbl` (see [`lab_common::table`]): numeric columns read right-aligned.
fn table<'a>(
    head: &[&'static str],
    numeric: &[usize],
    rows: Vec<Vec<Element<'a, Msg>>>,
) -> Element<'a, Msg> {
    let cols: Vec<lab_common::Col> = head
        .iter()
        .enumerate()
        .map(|(i, label)| lab_common::col(label, 1, numeric.contains(&i)))
        .collect();
    let rows: Vec<lab_common::Row<'a, Msg>> = rows
        .into_iter()
        .map(|cells| lab_common::Row {
            cells,
            on_press: None,
        })
        .collect();
    lab_common::table(&cols, rows)
}

/// A job's share as the web's `.lab-job-share`: a meter, then the percent.
fn share_cell<'a>(share: f64) -> Element<'a, Msg> {
    row![
        container(lab_common::meter(share, false)).width(Length::Fixed(96.0)),
        ui::sans(
            lab_common::percent(share),
            12.0,
            crate::theme::palette().ink_2
        )
        .width(Length::Fixed(44.0)),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

fn unknown<'a>(_id: &str) -> Element<'a, Msg> {
    lab_common::empty(
        "search",
        "No job with this id",
        None,
        Some(ui::btn(
            "Back to Jobs",
            None,
            ui::Kind::Secondary,
            Some(Msg::Go(Id::LabJobs)),
        )),
    )
}

// ---- The page --------------------------------------------------------------------------------

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let body: Element<'a, Msg> = if state.not_found {
        unknown(&state.job)
    } else if let Some(v) = &state.view {
        page(state, v)
    } else if let Some(err) = &state.error {
        ui::error_line(&format!("Could not load the job: {err}"))
    } else {
        ui::faint("Loading the job…").into()
    };
    lab::frame(
        Section::Jobs,
        ui::scroll(body).height(Length::Fill).into(),
        Msg::Go,
    )
}

fn page<'a>(state: &'a State, v: &'a JobView) -> Element<'a, Msg> {
    let live = !matches!(v.status.as_str(), "done" | "failed" | "cancelled");
    let now = lab_common::now_ms();
    let cancel: Element<'a, Msg> = if live {
        ui::btn_sm(
            "Cancel job",
            None,
            ui::Kind::Danger,
            (!state.cancelling).then_some(Msg::AskCancel),
        )
    } else {
        Space::new().into()
    };
    let head = row![
        ui::mono(v.id.clone(), 15.0, crate::theme::palette().ink),
        ui::badge(&cap(&v.status), tone_of(&v.status)),
        Space::new().width(Length::Fill),
        cancel,
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let stats = row![
        stat("Recipe", v.recipe.clone()),
        stat("Mode", v.mode.clone()),
        stat("Created", lab_common::ago_text(v.created_ms, now)),
        stat("Updated", lab_common::ago_text(v.updated_ms, now)),
    ]
    .spacing(12)
    .width(Length::Fill);

    let mut parts: Vec<Element<'a, Msg>> = vec![head.into(), stats.into()];
    if state.confirming {
        parts.push(confirm_strip(&v.id));
    }
    if let Some(err) = &state.cancel_error {
        parts.push(lab_common::err(&format!("The cancel failed: {err}")));
    }
    if let Some(err) = &state.error {
        parts.push(lab_common::err(&format!(
            "The job view is out of date: {err}"
        )));
    }
    let points = points_of(v, &state.live, &state.live_at);
    parts.push(grid2(
        ui::card("Status", timeline(&stages_of(v), now)),
        ui::card("Budget", budget_body(v)),
    ));
    parts.push(grid2(
        ui::card("Runs", runs_body(v)),
        ui::card("Charts", charts_body(&points)),
    ));
    parts.push(grid2(
        ui::card("Datasets", datasets_body(v)),
        ui::card("Plan", plan_body(v.plan.as_ref())),
    ));
    parts.push(ui::card("Devices", devices_body(&v.runs)));
    parts.push(ui::card_with(
        "Log tail",
        vec![lab_common::note(format!("{} events", state.logs.len()))],
        log_body(&state.logs),
    ));
    parts.push(ui::card("Outputs and metrics", outputs_body(v)));
    if live {
        parts.push(lab_common::note("This page follows the job live."));
    }
    column(parts).spacing(16).width(Length::Fill).into()
}

/// Two cards side by side, as the web's `.lab-grid2`: each keeps its own height.
fn grid2<'a>(left: Element<'a, Msg>, right: Element<'a, Msg>) -> Element<'a, Msg> {
    row![
        container(left).width(Length::FillPortion(1)),
        container(right).width(Length::FillPortion(1)),
    ]
    .spacing(16)
    .align_y(Alignment::Start)
    .width(Length::Fill)
    .into()
}

/// The cancel confirmation, as the web's dialog: the question, then keep running or cancel.
fn confirm_strip<'a>(id: &str) -> Element<'a, Msg> {
    container(
        column![
            ui::sans(
                format!(
                    "Cancel job {id}? Its runs stop; the GPU time used so far stays on the job."
                ),
                13.0,
                crate::theme::palette().ink
            ),
            row![
                ui::btn(
                    "Keep running",
                    None,
                    ui::Kind::Ghost,
                    Some(Msg::KeepRunning)
                ),
                ui::btn(
                    "Cancel job",
                    None,
                    ui::Kind::Danger,
                    Some(Msg::ConfirmCancel)
                ),
            ]
            .spacing(8),
        ]
        .spacing(10),
    )
    .padding(12)
    .width(Length::Fill)
    .style(|_: &Theme| container::Style {
        background: Some(crate::theme::palette().sheet_2.into()),
        border: Border::default().rounded(crate::theme::skin().radius),
        ..Default::default()
    })
    .into()
}

/// The plan the job was dispatched with, as the web's `planDiagram`; a note when none is recorded.
fn plan_body<'a>(plan: Option<&'a JobPlan>) -> Element<'a, Msg> {
    match plan {
        Some(plan) => super::lab_jobs::plan_view(plan),
        None => lab_common::note("No plan is recorded for this job."),
    }
}

/// The job's datasets by id, joined, or `none`.
fn datasets_body<'a>(v: &'a JobView) -> Element<'a, Msg> {
    let value = if v.datasets.is_empty() {
        "none".to_string()
    } else {
        v.datasets.join(", ")
    };
    ui::sans(value, 13.0, crate::theme::palette().ink).into()
}

/// The job's runs as the web's table: the run, its node and island, its devices, state, start and
/// end, device-hours and result.
fn runs_body<'a>(v: &'a JobView) -> Element<'a, Msg> {
    if v.runs.is_empty() {
        return lab_common::note("No runs yet");
    }
    let now = lab_common::now_ms();
    let rows: Vec<Vec<Element<'a, Msg>>> = v
        .runs
        .iter()
        .map(|r| {
            let devices = if r.devices.is_empty() {
                "cpu".to_string()
            } else {
                r.devices
                    .iter()
                    .map(|d| d.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            vec![
                lab_common::cell(r.node_job.clone(), true),
                txt(r.node.clone()),
                txt(r.island.clone()),
                txt(devices),
                txt(cap(&r.state)),
                txt(lab_common::ago_text(r.started_ms, now)),
                txt(match r.ended_ms {
                    Some(ended) if ended > 0 => lab_common::ago_text(ended, now),
                    _ => "—".to_string(),
                }),
                txt(dash2(r.gpu_hours)),
                txt(match r.exit_code {
                    Some(code) => format!("exit {code}"),
                    None => "—".to_string(),
                }),
            ]
        })
        .collect();
    table(
        &[
            "Run", "Node", "Island", "Devices", "State", "Started", "Ended", "GPU-h", "Result",
        ],
        &[7],
        rows,
    )
}

fn charts_body<'a>(points: &[SeriesPoint]) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if points.is_empty() {
        parts.push(
            ui::faint("Waiting for the trainer's first step. Queued jobs wait for a free device.")
                .into(),
        );
    }
    let specs = chart_specs(points);
    for pair in specs.chunks(2) {
        let mut cells: Vec<Element<'a, Msg>> = Vec::new();
        for spec in pair {
            let summary = ui::sans(spec.summary.clone(), 11.0, crate::theme::palette().ink_2);
            let body: Element<'a, Msg> = if spec.values.len() < 2 {
                summary.into()
            } else {
                column![
                    chart::stacked_area_with_heads(
                        vec![chart::Series {
                            key: spec.short.clone(),
                            color: spec.color,
                            values: spec.values.clone()
                        }],
                        spec.labels.clone(),
                        spec.heads.clone(),
                        spec.format,
                        "No points yet.",
                    ),
                    summary,
                ]
                .spacing(6)
                .width(Length::Fill)
                .into()
            };
            cells.push(container(body).width(Length::FillPortion(1)).into());
        }
        if pair.len() == 1 {
            cells.push(Space::new().width(Length::FillPortion(1)).into());
        }
        parts.push(row(cells).spacing(22).width(Length::Fill).into());
    }
    let rounds: Vec<&SeriesPoint> = points.iter().filter(|p| p.kind == "round").collect();
    if !rounds.is_empty() {
        let accepted = rounds.iter().filter(|p| p.accepted == Some(true)).count();
        parts.push(ui::faint(format!("Rounds: {}, accepted {accepted}.", rounds.len())).into());
        let rows = rounds
            .iter()
            .map(|p| {
                vec![
                    txt(p.round.map_or_else(|| "—".to_string(), |r| r.to_string())),
                    txt(p.island.clone()),
                    txt(match p.accepted {
                        None => "—",
                        Some(true) => "yes",
                        Some(false) => "no",
                    }),
                    txt(p
                        .hub_round
                        .map_or_else(|| "—".to_string(), |r| r.to_string())),
                ]
            })
            .collect();
        parts.push(table(
            &["Round", "Island", "Accepted", "Hub round"],
            &[0, 3],
            rows,
        ));
    }
    let trials: Vec<&SeriesPoint> = points
        .iter()
        .filter(|p| p.kind == "trial" && p.objective.is_some())
        .collect();
    if !trials.is_empty() {
        parts.push(ui::faint("Trials").into());
        let rows = trials
            .iter()
            .map(|p| {
                vec![
                    txt(p.trial.map_or_else(|| "—".to_string(), |t| t.to_string())),
                    txt(fix4(p.objective.unwrap_or(0.0))),
                ]
            })
            .collect();
        parts.push(table(&["Trial", "Objective"], &[0, 1], rows));
    }
    column(parts).spacing(22).width(Length::Fill).into()
}

fn devices_body<'a>(runs: &'a [JobRun]) -> Element<'a, Msg> {
    if runs.is_empty() {
        return ui::faint("No runs yet. A job gets its devices when it is dispatched.").into();
    }
    let total: f64 = runs.iter().map(|r| r.gpu_hours).sum();
    let mut rows: Vec<Vec<Element<'a, Msg>>> = Vec::new();
    for run in runs {
        let slots: Vec<Option<u32>> = if run.devices.is_empty() {
            vec![None]
        } else {
            run.devices.iter().map(|d| Some(*d)).collect()
        };
        let hours = run.gpu_hours / slots.len() as f64;
        for slot in slots {
            let device = match slot {
                None => "cpu".to_string(),
                Some(d) => format!("dev {d}"),
            };
            let share = if total > 0.0 { hours / total } else { 0.0 };
            rows.push(vec![
                txt(run.node.clone()),
                txt(device),
                txt(run.island.clone()),
                txt(run.state.clone()),
                txt(dash2(hours)),
                share_cell(share),
            ]);
        }
    }
    table(
        &["Node", "Device", "Island", "State", "GPU-h", "Share of job"],
        &[4],
        rows,
    )
}

fn log_body<'a>(logs: &'a [JobEvent]) -> Element<'a, Msg> {
    let lines: Vec<Element<'a, Msg>> = if logs.is_empty() {
        vec![lab_common::note("No events yet.")]
    } else {
        logs.iter()
            .map(|e| {
                ui::mono(
                    format!("#{} {} {}", e.seq, e.island, log_text(&e.event)),
                    12.0,
                    crate::theme::palette().ink_2,
                )
                .width(Length::Fill)
                .into()
            })
            .collect()
    };
    container(
        ui::scroll(column(lines).spacing(2).width(Length::Fill))
            .id(WidgetId::new(LOG_ID))
            .on_scroll(|viewport| Msg::LogScroll(viewport.relative_offset().y))
            .height(Length::Fixed(300.0)),
    )
    .padding(10)
    .width(Length::Fill)
    .style(|_: &Theme| container::Style {
        background: Some(crate::theme::palette().sheet_2.into()),
        border: Border::default().rounded(crate::theme::skin().radius),
        ..Default::default()
    })
    .into()
}

/// The budget as the web's card: the GPU-hours used of the budget, whether it is over, and the meter.
fn budget_body<'a>(v: &'a JobView) -> Element<'a, Msg> {
    let used = v.gpu_hours;
    let budget = v.budget_gpu_hours;
    let over = used > budget;
    let ratio = if budget > 0.0 { used / budget } else { 0.0 };
    let pairs: [(&str, String); 2] = [
        (
            "GPU-h used",
            format!("{} of {}", dash2(used), dash2(budget)),
        ),
        ("Over budget", if over { "yes" } else { "no" }.to_string()),
    ];
    let lines: Vec<Element<'a, Msg>> = pairs
        .into_iter()
        .map(|(label, value)| {
            row![
                container(ui::sans(
                    label.to_string(),
                    12.5,
                    crate::theme::palette().ink_3
                ))
                .width(Length::Fixed(140.0)),
                ui::sans(value, 13.0, crate::theme::palette().ink),
            ]
            .spacing(12)
            .align_y(Alignment::Center)
            .into()
        })
        .collect();
    column![column(lines).spacing(8), lab_common::meter(ratio, over),]
        .spacing(12)
        .width(Length::Fill)
        .into()
}

fn outputs_body<'a>(v: &'a JobView) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    let mut metrics: Vec<(&String, &Value)> = v
        .metrics
        .as_ref()
        .and_then(Value::as_object)
        .map(|m| m.iter().collect())
        .unwrap_or_default();
    metrics.sort_by(|a, b| a.0.cmp(b.0));
    if metrics.is_empty() {
        parts.push(ui::faint("No metrics yet.").into());
    } else {
        let rows = metrics
            .iter()
            .map(|(k, val)| vec![txt((*k).clone()), txt(metric_text(val))])
            .collect();
        parts.push(table(&["Metric", "Value"], &[1], rows));
    }
    parts.push(ui::faint("Outputs").into());
    if v.outputs.is_empty() {
        parts.push(ui::faint("No outputs yet.").into());
    } else {
        for o in &v.outputs {
            parts.push(ui::sans(o.clone(), 12.0, crate::theme::palette().ink_2).into());
        }
    }
    if v.outputs_remote {
        parts.push(ui::faint("The outputs stay on the node that trained them.").into());
    }
    column(parts).spacing(10).width(Length::Fill).into()
}

/// A status word with its first letter capitalised, as the web's `cap`.
fn cap(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The status timeline as the web's `ol.lab-steps`: a dot in each stage's tone, its label, when it
/// happened and any error under it.
fn timeline<'a>(stages: &[Stage], now: u64) -> Element<'a, Msg> {
    let steps: Vec<Element<'a, Msg>> = stages
        .iter()
        .map(|stage| {
            let tone = match stage.state {
                StageState::Done => crate::theme::palette().ok,
                StageState::Current => crate::theme::palette().info,
                StageState::Pending | StageState::Skipped => crate::theme::palette().ink_3,
            };
            let when = match stage.at {
                Some(at) => lab_common::ago_text(at, now),
                None => "—".to_string(),
            };
            let dot = container(
                Space::new()
                    .width(Length::Fixed(8.0))
                    .height(Length::Fixed(8.0)),
            )
            .style(move |_: &Theme| container::Style {
                background: Some(tone.into()),
                border: Border::default().rounded(4.0),
                ..Default::default()
            })
            .padding(Padding {
                top: 5.0,
                right: 0.0,
                bottom: 0.0,
                left: 0.0,
            });
            let mut lines = column![
                row![
                    text(stage.label.to_string())
                        .font(SANS_SEMIBOLD)
                        .size(13.0)
                        .color(crate::theme::palette().ink),
                    lab_common::note(when),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            ]
            .spacing(2);
            if !stage.sub.is_empty() {
                lines = lines.push(ui::sans(
                    stage.sub.clone(),
                    12.5,
                    crate::theme::palette().crit,
                ));
            }
            row![dot, lines].spacing(10).width(Length::Fill).into()
        })
        .collect();
    column(steps).spacing(12).width(Length::Fill).into()
}

/// The cancel path, driven through the view: the header's Cancel button, then the confirmation's
/// Cancel button, against a local fixture hub (not the apid). The hub answers the job's GET and its
/// cancel POST; the cancel call itself runs here, where the iced task would run it.
#[cfg(test)]
mod cancel_check {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    use iced_test::selector::{Candidate, Selector};

    use super::*;

    /// The second (or nth) text match of `text`, in the widget tree's order.
    struct NthText {
        text: &'static str,
        nth: usize,
        seen: usize,
    }

    impl Selector for NthText {
        type Output = iced_test::selector::Text;

        fn select(&mut self, candidate: Candidate<'_>) -> Option<Self::Output> {
            let mut text = self.text;
            let found = text.select(candidate)?;
            let index = self.seen;
            self.seen += 1;
            (index == self.nth).then_some(found)
        }

        fn description(&self) -> String {
            format!("text {:?} (match {})", self.text, self.nth)
        }
    }

    fn job_json(status: &str) -> String {
        serde_json::json!({
            "id": "tj-cancel", "status": status, "mode": "single", "recipe": "vision-small@1.2.0",
            "datasets": [], "created_ms": 1, "updated_ms": 2, "gpu_hours": 0.1, "budget_gpu_hours": 1.0,
            "error": null, "plan": null, "runs": [], "series": [], "metrics": null, "outputs": [],
            "outputs_remote": false, "seq": 0
        })
        .to_string()
    }

    /// A one-thread hub answering the three routes this check uses; returns its base URL.
    fn serve_fixture() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the fixture hub");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        let status = Arc::new(Mutex::new("running".to_string()));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                loop {
                    let n = match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let line = head.lines().next().unwrap_or_default().to_string();
                let body = if line.starts_with("POST /v1/foundry/jobs/tj-cancel/cancel") {
                    *status.lock().unwrap() = "cancelled".to_string();
                    job_json("cancelled")
                } else if line.starts_with("GET /v1/foundry/jobs/tj-cancel/events") {
                    r#"{"job":"tj-cancel","events":[],"next":0}"#.to_string()
                } else if line.starts_with("GET /v1/foundry/jobs/tj-cancel") {
                    job_json(&status.lock().unwrap())
                } else {
                    r#"{"error":"no such job"}"#.to_string()
                };
                let code = if body.contains("no such job") {
                    "404 Not Found"
                } else {
                    "200 OK"
                };
                let reply = format!(
                    "HTTP/1.1 {code}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        url
    }

    /// Presses the first match of `selector` in the view, and runs the messages it produced.
    fn press<S>(state: &mut State, ctx: &Ctx, selector: S)
    where
        S: Selector + Send,
        S::Output: iced_test::selector::Bounded + Clone + Send + Sync + 'static,
    {
        let messages: Vec<Msg> = {
            let mut ui = iced_test::simulator(view(state, ctx));
            ui.click(selector)
                .expect("the button is in the view and visible");
            ui.into_messages().collect()
        };
        for msg in messages {
            let _ = update(state, msg, ctx);
        }
    }

    #[test]
    fn the_cancel_buttons_cancel_the_job_on_the_hub() {
        let url = serve_fixture();
        let ctx = Ctx {
            client: client::Client::new(&url, None, None),
            project: None,
        };
        let snap = fetch(&ctx.client, "tj-cancel", 0).expect("the fixture job loads");
        let mut state = State {
            job: "tj-cancel".to_string(),
            ..Default::default()
        };
        let _ = update(&mut state, Msg::Loaded(Ok(snap)), &ctx);
        assert_eq!(
            state.view.as_ref().map(|v| v.status.as_str()),
            Some("running")
        );

        // The header's Cancel job button opens the confirmation (the first match in the view).
        press(&mut state, &ctx, "Cancel job");
        assert!(state.confirming, "the header button opens the confirmation");

        // The confirmation's Cancel job button is the second match; its message is ConfirmCancel.
        press(
            &mut state,
            &ctx,
            NthText {
                text: "Cancel job",
                nth: 1,
                seen: 0,
            },
        );
        assert!(
            !state.confirming,
            "confirming closes once the cancel is asked for"
        );
        assert!(state.cancelling, "the cancel request is in flight");

        // The task the update returned makes the call; run that call here and feed back its answer.
        let answer = lab_api::cancel_job(&ctx.client, "tj-cancel");
        assert!(answer.is_ok(), "the hub answers the cancel: {answer:?}");
        let _ = update(&mut state, Msg::Cancelled(answer), &ctx);
        assert!(!state.cancelling);

        let after = fetch(&ctx.client, "tj-cancel", 0).expect("reloads after the cancel");
        assert_eq!(after.view.status, "cancelled");
    }
}
