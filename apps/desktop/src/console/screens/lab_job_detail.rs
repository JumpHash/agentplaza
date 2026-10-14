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
use iced::widget::{
    Id as WidgetId, Space, button, column, container, rich_text, row, scrollable, span,
};
use iced::{Alignment, Border, Color, Element, Length, Task, Theme};
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
use crate::theme::SANS;

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
/// The web's chart palette in its dark theme (chart-3 to chart-8).
fn palette() -> [Color; 6] {
    [
        crate::theme::palette().warn,
        Color::from_rgb8(0xb7, 0x9c, 0xf0),
        Color::from_rgb8(0x5a, 0xd6, 0xd6),
        Color::from_rgb8(0xd9, 0xa8, 0x77),
        crate::theme::palette().ink_2,
        crate::theme::palette().crit,
    ]
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

fn stage_cell<'a>(stage: &Stage) -> Element<'a, Msg> {
    let bar = match stage.state {
        StageState::Done => crate::theme::palette().ok,
        StageState::Current => crate::theme::palette().signal,
        StageState::Pending | StageState::Skipped => crate::theme::palette().rule,
    };
    let mut lines = column![
        ui::sans(stage.label, 12.0, crate::theme::palette().ink),
        ui::sans(
            lab_common::time(stage.at.unwrap_or(0)),
            11.0,
            crate::theme::palette().ink_3
        )
    ]
    .spacing(4);
    if !stage.sub.is_empty() {
        lines = lines.push(ui::faint(stage.sub.clone()));
    }
    row![
        container(Space::new().width(3.0).height(44.0)).style(move |_: &Theme| container::Style {
            background: Some(bar.into()),
            ..Default::default()
        }),
        lines,
    ]
    .spacing(10)
    .width(Length::Fill)
    .into()
}

// ---- Shared pieces ---------------------------------------------------------------------------

fn txt<'a>(content: impl Into<String>) -> Element<'a, Msg> {
    ui::sans(content.into(), 12.5, crate::theme::palette().ink).into()
}

fn stat<'a>(label: &str, value: String) -> Element<'a, Msg> {
    container(
        column![
            ui::label(label),
            ui::sans(value, 14.0, crate::theme::palette().ink)
        ]
        .spacing(6),
    )
    .padding([12, 16])
    .width(Length::Fill)
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

/// A table over equal cells: the header in mono caps, numeric columns right-aligned, a hairline under each row.
fn table<'a>(
    head: &[&str],
    numeric: &[usize],
    rows: Vec<Vec<Element<'a, Msg>>>,
) -> Element<'a, Msg> {
    let cell = |content: Element<'a, Msg>, i: usize| -> Element<'a, Msg> {
        let align = if numeric.contains(&i) {
            Alignment::End
        } else {
            Alignment::Start
        };
        container(content)
            .padding([6, 8])
            .width(Length::FillPortion(1))
            .align_x(align)
            .into()
    };
    let mut lines = column![
        row(head
            .iter()
            .enumerate()
            .map(|(i, label)| cell(ui::label(label).into(), i))
            .collect::<Vec<_>>())
        .width(Length::Fill)
    ]
    .spacing(0);
    for cells in rows {
        let line = row(cells
            .into_iter()
            .enumerate()
            .map(|(i, c)| cell(c, i))
            .collect::<Vec<_>>())
        .width(Length::Fill);
        lines = lines.push(line).push(ui::hairline::<Msg>());
    }
    lines.into()
}

fn share_cell<'a>(share: f64) -> Element<'a, Msg> {
    let pct = (share.clamp(0.0, 1.0) * 100.0).round() as u16;
    row![
        container(Space::new().width(Length::Fill).height(6.0))
            .width(Length::FillPortion(pct.max(1)))
            .style(|_: &Theme| container::Style {
                background: Some(crate::theme::palette().signal.into()),
                ..Default::default()
            }),
        container(Space::new().width(Length::Fill).height(6.0))
            .width(Length::FillPortion((100 - pct).max(1)))
            .style(|_: &Theme| container::Style {
                background: Some(crate::theme::palette().rule_faint.into()),
                ..Default::default()
            }),
        ui::sans(
            lab_common::percent(share),
            11.0,
            crate::theme::palette().ink_2
        )
        .width(Length::Fixed(44.0)),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

fn unknown<'a>(id: &str) -> Element<'a, Msg> {
    let title = if id.is_empty() {
        "Job".to_string()
    } else {
        format!("Job {id}")
    };
    column![
        ui::empty(
            "brain",
            &title,
            "No job with this id. Go back to Jobs to pick one."
        ),
        container(back_link())
            .width(Length::Fill)
            .align_x(Alignment::Center),
    ]
    .spacing(8)
    .into()
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
    let terminal = matches!(v.status.as_str(), "done" | "failed" | "cancelled");
    let cancellable = matches!(v.status.as_str(), "queued" | "running") && !state.cancelling;
    let cancel = ui::btn(
        "Cancel job",
        None,
        ui::Kind::Danger,
        cancellable.then_some(Msg::AskCancel),
    );
    let head = row![
        ui::mono(format!("Job {}", v.id), 22.0, crate::theme::palette().ink),
        Space::new().width(Length::Fill),
        ui::badge(&v.status, tone_of(&v.status)),
        cancel,
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    let stats = row![
        stat("Recipe", v.recipe.clone()),
        stat("Mode", v.mode.clone()),
        stat("Created", lab_common::time(v.created_ms)),
        stat("Updated", lab_common::time(v.updated_ms)),
    ]
    .spacing(12);

    let mut parts: Vec<Element<'a, Msg>> = vec![head.into(), stats.into()];
    if state.confirming {
        parts.push(confirm_strip(&v.id));
    }
    if let Some(err) = &state.cancel_error {
        parts.push(ui::error_line(&format!("The cancel failed: {err}")));
    }
    if let Some(err) = &state.error {
        parts.push(ui::error_line(&format!(
            "The job view is out of date: {err}"
        )));
    }
    parts.push(ui::card(
        "Status",
        row(stages_of(v).iter().map(stage_cell).collect::<Vec<_>>())
            .spacing(12)
            .width(Length::Fill)
            .into(),
    ));
    parts.push(ui::card("Plan", plan_body(v.plan.as_ref())));

    let points = points_of(v, &state.live, &state.live_at);
    parts.push(ui::card("Charts", charts_body(&points)));

    parts.push(ui::card("Devices", devices_body(&v.runs)));
    parts.push(ui::card("Log tail", log_body(&state.logs)));

    let budget = budget_body(v);
    let outputs = outputs_body(v);
    parts.push(
        row![
            ui::card("Budget", budget),
            ui::card("Outputs and metrics", outputs)
        ]
        .spacing(22)
        .width(Length::Fill)
        .into(),
    );

    let error = match &v.error {
        Some(err) => ui::error_line(err),
        None => ui::faint("No error.").into(),
    };
    parts.push(ui::card("Error", error));
    if !terminal {
        parts.push(ui::faint("This page follows the job live.").into());
    }
    column(parts).spacing(22).width(Length::Fill).into()
}

fn confirm_strip<'a>(id: &str) -> Element<'a, Msg> {
    container(
        row![
            ui::faint(format!(
                "Cancel job {id}? Its runs stop; the GPU time used so far stays on the job."
            ))
            .width(Length::Fill),
            ui::btn(
                "Keep running",
                None,
                ui::Kind::Secondary,
                Some(Msg::KeepRunning)
            ),
            ui::btn(
                "Cancel job",
                None,
                ui::Kind::Danger,
                Some(Msg::ConfirmCancel)
            ),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    )
    .padding(12)
    .width(Length::Fill)
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

fn plan_body<'a>(plan: Option<&'a JobPlan>) -> Element<'a, Msg> {
    let Some(plan) = plan else {
        return ui::faint("No plan is recorded for this job.").into();
    };
    let mode = if plan.mode == "loose" {
        format!(
            "Loose: islands train {} inner steps, then average through the hub. Rounds {}, staleness {}.",
            plan.inner_steps,
            plan.rounds
                .map_or_else(|| "—".to_string(), |r| r.to_string()),
            plan.staleness
        )
    } else if plan.mode == "tight" {
        "Tight: the machines of each island train together and sync through the island's rendezvous.".to_string()
    } else {
        "Single: one trainer on one machine.".to_string()
    };
    let islands: Vec<Element<'a, Msg>> = plan.islands.iter().map(island_cell).collect();
    let islands: Element<'a, Msg> = if islands.is_empty() {
        ui::faint("No islands.").into()
    } else {
        row(islands).spacing(12).wrap().vertical_spacing(12).into()
    };
    let rounds = plan
        .rounds
        .map_or_else(|| "—".to_string(), |r| r.to_string());
    let meta: Vec<Element<'a, Msg>> = vec![
        ui::sans(
            format!("Devices {}", plan.devices_total),
            11.0,
            crate::theme::palette().ink_3,
        )
        .into(),
        ui::sans(
            format!("Inner steps {}", plan.inner_steps),
            11.0,
            crate::theme::palette().ink_3,
        )
        .into(),
        ui::sans(
            format!("Staleness {}", plan.staleness),
            11.0,
            crate::theme::palette().ink_3,
        )
        .into(),
        ui::sans(
            format!("Rounds {rounds}"),
            11.0,
            crate::theme::palette().ink_3,
        )
        .into(),
    ];
    let datasets = if plan.datasets.is_empty() {
        "—".to_string()
    } else {
        plan.datasets
            .iter()
            .map(|(name, id)| format!("{name} {id}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let kv = |label: &str, value: String| -> Element<'a, Msg> {
        row![
            ui::faint(label.to_string()).width(Length::Fixed(96.0)),
            ui::sans(value, 11.5, crate::theme::palette().ink_2).width(Length::Fill)
        ]
        .spacing(10)
        .align_y(Alignment::Center)
        .into()
    };
    column![
        ui::prose(mode),
        islands,
        row(meta).spacing(18).wrap().vertical_spacing(6),
        column![
            kv("Datasets", datasets),
            kv("Frozen", non_empty(plan.frozen.as_deref())),
            kv("Output", non_empty(Some(plan.output_dir.as_str()))),
        ]
        .spacing(4),
    ]
    .spacing(10)
    .width(Length::Fill)
    .into()
}

/// The web's `.lab-job-back`: a mono link in the signal ink, underlined.
fn back_link<'a>() -> Element<'a, Msg> {
    let spans: Vec<iced::widget::text::Span<'a>> = vec![
        span("Back to Jobs")
            .font(SANS)
            .size(13.0)
            .color(crate::theme::palette().signal_ink)
            .underline(true),
    ];
    let label: Element<'a, Msg> = rich_text(spans).into();
    button(label)
        .on_press(Msg::Go(Id::LabJobs))
        .padding(0)
        .style(|_: &Theme, _: button::Status| button::Style {
            background: None,
            text_color: crate::theme::palette().signal_ink,
            ..Default::default()
        })
        .into()
}

fn non_empty(s: Option<&str>) -> String {
    match s {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => "—".to_string(),
    }
}

fn island_cell<'a>(isl: &'a lab_api::Island) -> Element<'a, Msg> {
    let machines: Vec<Element<'a, Msg>> = isl
        .machines
        .iter()
        .map(|m| {
            let devices: Vec<Element<'a, Msg>> = if m.devices.is_empty() {
                vec![ui::faint("no devices").into()]
            } else {
                m.devices
                    .iter()
                    .map(|d| {
                        container(ui::mono(
                            format!("dev {d}"),
                            11.0,
                            crate::theme::palette().ink_2,
                        ))
                        .padding([1, 6])
                        .style(|_: &Theme| container::Style {
                            border: Border {
                                color: crate::theme::palette().rule,
                                width: 1.0,
                                radius: 0.0.into(),
                            },
                            ..Default::default()
                        })
                        .into()
                    })
                    .collect()
            };
            row![
                ui::mono(m.id.clone(), 12.0, crate::theme::palette().ink),
                row(devices).spacing(8).align_y(Alignment::Center)
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        })
        .collect();
    let mut lines = column![
        row![
            ui::mono(
                format!("island {}", isl.id),
                12.5,
                crate::theme::palette().ink
            ),
            ui::badge(
                non_empty(Some(isl.backend.as_str())).as_str(),
                ui::Tone::Signal
            )
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        row![
            ui::faint("Leader"),
            ui::sans(
                non_empty(Some(isl.leader.as_str())),
                11.0,
                crate::theme::palette().ink_2
            )
        ]
        .spacing(10),
        row![
            ui::faint("Rendezvous"),
            ui::sans(
                non_empty(Some(isl.rdzv.as_str())),
                11.0,
                crate::theme::palette().ink_2
            )
        ]
        .spacing(10),
    ]
    .spacing(6);
    for m in machines {
        lines = lines.push(container(m).padding([6, 0]));
    }
    container(lines)
        .padding(12)
        .width(Length::Fixed(320.0))
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
    if logs.is_empty() {
        return ui::faint("No events yet.").into();
    }
    let lines: Vec<Element<'a, Msg>> = logs
        .iter()
        .map(|e| {
            ui::mono(
                format!("#{} {} {}", e.seq, e.island, log_text(&e.event)),
                11.5,
                crate::theme::palette().ink_2,
            )
            .width(Length::Fill)
            .into()
        })
        .collect();
    ui::scroll(column(lines).spacing(2).width(Length::Fill))
        .id(WidgetId::new(LOG_ID))
        .on_scroll(|viewport| Msg::LogScroll(viewport.relative_offset().y))
        .height(Length::Fixed(320.0))
        .into()
}

fn budget_body<'a>(v: &'a JobView) -> Element<'a, Msg> {
    let used = v.gpu_hours;
    let budget = v.budget_gpu_hours;
    let ratio = if budget > 0.0 { used / budget } else { 0.0 };
    let pct = (ratio.clamp(0.0, 1.0) * 100.0).round() as u16;
    let fill = if used > budget {
        crate::theme::palette().crit
    } else {
        crate::theme::palette().signal
    };
    let bar = row![
        container(Space::new().width(Length::Fill).height(10.0))
            .width(Length::FillPortion(pct.max(1)))
            .style(move |_: &Theme| container::Style {
                background: Some(fill.into()),
                ..Default::default()
            }),
        container(Space::new().width(Length::Fill).height(10.0))
            .width(Length::FillPortion((100 - pct).max(1)))
            .style(|_: &Theme| container::Style {
                background: Some(crate::theme::palette().rule_faint.into()),
                ..Default::default()
            }),
    ];
    column![
        ui::sans(
            format!(
                "{} of {} GPU-h used ({})",
                dash2(used),
                dash2(budget),
                lab_common::percent(ratio)
            ),
            12.5,
            crate::theme::palette().ink
        ),
        bar,
    ]
    .spacing(8)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn live_lines_past_the_view_seq_are_the_only_ones_added() {
        let view = JobView {
            seq: 3,
            series: vec![SeriesPoint {
                kind: "step".into(),
                island: "a".into(),
                step: Some(1),
                loss: Some(2.0),
                ..Default::default()
            }],
            ..Default::default()
        };
        let frame = |seq: u64, step: u64| TrainingFrame {
            job: "j".into(),
            line: json!({"seq": seq, "island": "a", "node": "n", "event": {"ev": "step", "step": step, "loss": 1.5}}),
        };
        let live = vec![frame(2, 9), frame(4, 2), frame(5, 3), frame(5, 3)];
        let live_at: Vec<u64> = live.iter().map(|_| 0).collect();
        let points = points_of(&view, &live, &live_at);
        let steps: Vec<Option<u64>> = points.iter().map(|p| p.step).collect();
        assert_eq!(steps, vec![Some(1), Some(2), Some(3)]);
    }

    #[test]
    fn log_lines_sort_their_fields_and_end_in_the_name() {
        let event = json!({"ev": "step", "step": 40, "loss": 0.25, "lr": 0.0003, "extra": {"b": 1, "a": "x"}});
        assert_eq!(
            log_text(&event),
            "step extra={\"a\":\"x\",\"b\":1} loss=0.25 lr=0.0003 step=40"
        );
    }

    #[test]
    fn exponent_and_fixed_formats_match_the_web() {
        assert_eq!(exp2(0.0003), "3.00e-4");
        assert_eq!(fix4(0.4321_f64), "0.4321");
    }
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
