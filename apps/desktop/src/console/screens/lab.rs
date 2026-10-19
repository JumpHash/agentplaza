//! The Lab (sidebar `Lab`): the training lab's sections, parity with the web's `#/lab`. This module
//! holds what every Lab screen shares, the page head and the section strip ([`frame`]) and [`Section`],
//! and it is the Overview: the GPUs across the fleet (a card per machine, its devices and what holds
//! each one), the queue, the recent models and the week's training GPU-hours with a daily bar chart.
//! The web draws the same parts in the same order with the same labels and empty states. The other Lab
//! screens are `lab_jobs`, `lab_job_detail`, `lab_recipes`, `lab_datasets`, `lab_models`,
//! `lab_model_detail` and `lab_breeders`; each draws its body inside [`frame`] under its [`Section`].
//!
//! The Overview reloads on the console's 4 s tick and on every `training` line (one load at a time),
//! and its GPU-hours figure and bars settle over a short animation, as the web's count-up does.

use std::time::{Duration, Instant};

use chrono::{Days, Local, NaiveDate, TimeZone};
use iced::widget::{Space, button, column, container, row, text};
use iced::{
    Alignment, Border, Color, Element, Length, Padding, Subscription, Task, Theme, alignment,
};

use super::Id;
use super::lab_common::{self as common, Row, cell, col, note};
use crate::console::chart;
use crate::console::client::{self, Client};
use crate::console::lab_api::{self, Device, DeviceMachine, JobSummary, ModelView};
use crate::console::{Ctx, ui};
use crate::theme::{SANS, SANS_SEMIBOLD, TYPE_META, TYPE_SECTION};

/// A section tab. A detail screen sits under its list's tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Overview,
    Jobs,
    Recipes,
    Datasets,
    Models,
    Breeders,
}

/// The section strip, in the web's order.
pub const SECTIONS: &[(Section, &str)] = &[
    (Section::Overview, "Overview"),
    (Section::Jobs, "Jobs"),
    (Section::Recipes, "Recipes"),
    (Section::Datasets, "Datasets"),
    (Section::Models, "Models"),
    (Section::Breeders, "Breeders"),
];

impl Section {
    /// The screen this tab opens (a list; the detail screens are opened by their rows).
    pub fn id(self) -> Id {
        match self {
            Section::Overview => Id::Lab,
            Section::Jobs => Id::LabJobs,
            Section::Recipes => Id::LabRecipes,
            Section::Datasets => Id::LabDatasets,
            Section::Models => Id::LabModels,
            Section::Breeders => Id::LabBreeders,
        }
    }

    /// The tab a Lab screen sits under, or None for a screen outside the Lab.
    pub fn of(id: &Id) -> Option<Section> {
        Some(match id {
            Id::Lab => Section::Overview,
            Id::LabJobs | Id::LabJobDetail(_) => Section::Jobs,
            Id::LabRecipes => Section::Recipes,
            Id::LabDatasets => Section::Datasets,
            Id::LabModels | Id::LabModelDetail(_) => Section::Models,
            Id::LabBreeders => Section::Breeders,
            _ => return None,
        })
    }
}

/// A Lab screen's frame, as the web's Lab page: the page head ("Lab"), the section strip, then the
/// section's body (a scroll the section makes, so its own id can be scrolled to). A tab sends `on_go` of the screen it opens.
pub fn frame<'a, M: Clone + 'a>(
    current: Section,
    body: Element<'a, M>,
    on_go: impl Fn(Id) -> M + 'a,
) -> Element<'a, M> {
    let strip = lab_tabs(current, move |section: Section| on_go(section.id()));
    ui::sheet(
        ui::view_head("Lab", Vec::new()),
        column![strip, body].spacing(16).height(Length::Fill).into(),
    )
}

// ---- Overview --------------------------------------------------------------

const MACHINES_PER_ROW: usize = 3;
/// The height of the day bars' area, in pixels; the tallest day fills it.
const BAR_AREA: f32 = 120.0;
/// The smallest bar drawn, as the web's `min-height: 2px`.
const BAR_MIN: f32 = 2.0;
/// How long the GPU-hours figure and bars take to settle after a load.
const ANIM: Duration = Duration::from_millis(600);
/// The GPU-hours window: the last seven local days, today included.
const WEEK_DAYS: usize = 7;
const MODELS_SHOWN: usize = 6;

/// The last seven days, from the fetch: the day labels (the weekday, short), each day's training
/// GPU-hours and the number of jobs created in the window.
#[derive(Debug, Clone, Default)]
pub struct Week {
    labels: Vec<String>,
    gpu_hours: Vec<f64>,
    jobs: usize,
}

/// What one load brings: the fleet, every job, the registered models and the week.
#[derive(Debug, Clone, Default)]
pub struct Data {
    machines: Vec<DeviceMachine>,
    jobs: Vec<JobSummary>,
    models: Vec<ModelView>,
    week: Week,
}

/// A figure settling from the values on screen to new ones over [`ANIM`].
#[derive(Debug, Clone)]
struct Figures {
    from: Vec<f64>,
    to: Vec<f64>,
    start: Instant,
}

impl Figures {
    /// The values at `now`: eased from `from` to `to`.
    fn at(&self, now: Instant) -> Vec<f64> {
        let t = now.saturating_duration_since(self.start).as_secs_f64() / ANIM.as_secs_f64();
        let e = chart::ease(t);
        self.to
            .iter()
            .enumerate()
            .map(|(i, &to)| {
                let from = self.from.get(i).copied().unwrap_or(to);
                from + (to - from) * e
            })
            .collect()
    }

    fn running(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.start) < ANIM
    }
}

#[derive(Debug, Default)]
pub struct State {
    data: Option<Data>,
    error: Option<String>,
    /// A load is in flight; a line or tick that asks for another waits for the next.
    busy: bool,
    gpu_hours: Option<Figures>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    /// A training line arrived: the queue and the utilization may have moved.
    Live,
    /// A frame of the settling animation.
    Frame,
    /// A load finished.
    Loaded(Result<Data, String>),
}

/// The navigation a message asks for.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// Reloads the Overview (on open and on every poll tick); a load already in flight is not doubled.
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    if state.busy {
        return Task::none();
    }
    state.busy = true;
    let c = ctx.client.clone();
    client::task(move || fetch(&c), Msg::Loaded)
}

fn fetch(c: &Client) -> Result<Data, String> {
    let machines = lab_api::list_devices(c)?.machines;
    let jobs = lab_api::list_jobs(c)?;
    let models = lab_api::list_models(c)?;
    let week = week(&jobs, Local::now().date_naive());
    Ok(Data {
        machines,
        jobs,
        models,
        week,
    })
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) | Msg::Frame => Task::none(),
        Msg::Live => load(state, ctx),
        Msg::Loaded(result) => {
            state.busy = false;
            match result {
                Ok(data) => {
                    let now = Instant::now();
                    let to = data.week.gpu_hours.clone();
                    // The first load draws its figures as they are; later ones settle from what is on screen.
                    let from = match (&state.gpu_hours, &state.data) {
                        (Some(figures), Some(_)) => figures.at(now),
                        _ => to.clone(),
                    };
                    state.gpu_hours = Some(Figures {
                        from,
                        to,
                        start: now,
                    });
                    state.data = Some(data);
                    state.error = None;
                }
                Err(err) => state.error = Some(err),
            }
            Task::none()
        }
    }
}

/// The training lines (queue and utilization move with them) and the settling animation's frames.
pub fn subscription(state: &State, ctx: &Ctx) -> Subscription<Msg> {
    let live = common::live(&ctx.client)
        .filter_map(|e| common::training_frame(&e))
        .map(|_| Msg::Live);
    let animating = state
        .gpu_hours
        .as_ref()
        .is_some_and(|f| f.running(Instant::now()));
    if animating {
        Subscription::batch([
            live,
            iced::time::every(Duration::from_millis(33)).map(|_| Msg::Frame),
        ])
    } else {
        live
    }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let now = Instant::now();
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(err) = &state.error {
        parts.push(common::err(err));
    }
    match &state.data {
        None => {
            parts.push(fleet_section(String::new(), note("Reading the lab…")));
            parts.push(ui::card_with("Queue", Vec::new(), note("Reading the lab…")));
            parts.push(
                row![
                    ui::card_with("Recent models", Vec::new(), note("Reading the lab…")),
                    ui::card_with(
                        "Training GPU-hours this week",
                        Vec::new(),
                        column![common::figure("—")]
                            .spacing(12)
                            .width(Length::Fill)
                            .into(),
                    ),
                ]
                .spacing(16)
                .width(Length::Fill)
                .into(),
            );
        }
        Some(d) => {
            parts.push(fleet_card(d));
            parts.push(queue_card(d));
            parts.push(
                row![
                    models_card(d),
                    hours_card(d, state.gpu_hours.as_ref().map(|f| f.at(now))),
                ]
                .spacing(16)
                .width(Length::Fill)
                .into(),
            );
        }
    }
    frame(
        Section::Overview,
        ui::scroll(column(parts).spacing(16).width(Length::Fill))
            .height(Length::Fill)
            .into(),
        Msg::Go,
    )
}

// ---- Strip -----------------------------------------------------------------

/// The tab label's size, and Martian Mono's advance as a share of the size (see [`lab_tabs`]).
const TAB_SIZE: f32 = 11.5;
const TAB_ADVANCE: f32 = 0.7;

/// The section strip as the web's `.tabs`: mono caps at their natural width, 22 apart, the chosen
/// tab underlined in signal and a rule under the whole strip. The shared `ui::tabs` spreads its tabs
/// across the width, so the Lab draws its own.
fn lab_tabs<'a, M: Clone + 'a>(
    current: Section,
    on_go: impl Fn(Section) -> M + 'a,
) -> Element<'a, M> {
    let mut strip = row![].spacing(22);
    for &(value, name) in SECTIONS {
        let on = value == current;
        let bar = if on {
            crate::theme::palette().signal
        } else {
            Color::TRANSPARENT
        };
        // Martian Mono is monospace (advance 0.7 em), so a label is as wide as its characters: the
        // underline is drawn that wide, not to Fill, which would share the strip out across the width.
        let width = name.chars().count() as f32 * TAB_ADVANCE * TAB_SIZE;
        let body = column![
            container(text(name.to_string()).font(SANS_SEMIBOLD).size(TAB_SIZE)).padding(Padding {
                top: 10.0,
                right: 0.0,
                bottom: 8.0,
                left: 0.0
            }),
            container(Space::new().width(Length::Fixed(width)).height(2.0)).style(
                move |_: &Theme| container::Style {
                    background: Some(bar.into()),
                    ..Default::default()
                }
            ),
        ];
        // The text carries no colour of its own: it takes the button's state colour.
        let cell = button(body)
            .padding(0.0)
            .style(move |_: &Theme, status: button::Status| {
                let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
                button::Style {
                    background: None,
                    text_color: if on || hovered {
                        crate::theme::palette().ink
                    } else {
                        crate::theme::palette().ink_3
                    },
                    border: Border::default(),
                    ..Default::default()
                }
            })
            .on_press(on_go(value));
        strip = strip.push(cell);
    }
    column![strip, common::rule(crate::theme::palette().rule)]
        .width(Length::Fill)
        .into()
}

// ---- Fleet -----------------------------------------------------------------

/// The fleet's heading (the web's `h3.lab-h` with its meta) over its body.
fn fleet_section<'a>(meta: String, body: Element<'a, Msg>) -> Element<'a, Msg> {
    let head = row![
        text("GPUs across the fleet")
            .font(SANS_SEMIBOLD)
            .size(TYPE_SECTION)
            .color(crate::theme::palette().ink),
        common::note(meta),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .width(Length::Fill);
    column![head, body].spacing(8).width(Length::Fill).into()
}

/// The GPUs across the fleet: a card per machine, in rows of three, or the empty state.
fn fleet_card(d: &Data) -> Element<'_, Msg> {
    let devices: usize = d.machines.iter().map(|m| m.devices.len()).sum();
    let busy: usize = d
        .machines
        .iter()
        .flat_map(|m| m.devices.iter())
        .filter(|dv| is_busy(dv))
        .count();
    let meta = format!(
        "{} · {} busy",
        plural(devices as u64, "GPU"),
        common::count(busy as u64)
    );
    let body: Element<'_, Msg> = if devices == 0 {
        ui::empty(
            "cpu",
            "No GPUs reported",
            "Check the machine's noded trainer: a machine reports its GPUs once its trainer is up.",
        )
    } else {
        let rows: Vec<Element<'_, Msg>> = d
            .machines
            .chunks(MACHINES_PER_ROW)
            .map(|chunk| {
                let mut cards: Vec<Element<'_, Msg>> = chunk.iter().map(machine_card).collect();
                while cards.len() < MACHINES_PER_ROW {
                    cards.push(Space::new().width(Length::Fill).into());
                }
                row(cards).spacing(12).width(Length::Fill).into()
            })
            .collect();
        column(rows).spacing(12).width(Length::Fill).into()
    };
    fleet_section(meta, body)
}

/// One machine, as the web's card: its id, its online and trainer badges, a count line, then each
/// device, each set off from the one before by a hairline.
fn machine_card(m: &DeviceMachine) -> Element<'_, Msg> {
    let busy = m.devices.iter().filter(|dv| is_busy(dv)).count();
    let badges: Vec<Element<'_, Msg>> = vec![
        if m.online {
            ui::badge_dot("online", ui::Tone::Ok)
        } else {
            ui::badge_dot("offline", ui::Tone::Muted)
        },
        ui::badge(
            if m.trainer_ok {
                "trainer up"
            } else {
                "trainer down"
            },
            if m.trainer_ok {
                ui::Tone::Ok
            } else {
                ui::Tone::Warn
            },
        ),
    ];
    let mut lines: Vec<Element<'_, Msg>> = vec![note(format!(
        "{} · {} busy",
        plural(m.devices.len() as u64, "GPU"),
        common::count(busy as u64)
    ))];
    if m.devices.is_empty() {
        lines.push(note("No GPUs reported"));
    } else {
        for dv in &m.devices {
            lines.push(device_row(dv));
        }
    }
    let body = column(lines).spacing(0).width(Length::Fill);
    ui::card_with(&m.id, badges, body.into())
}

/// One device, as the web's `.lab-dev`: its name and backend, the vendor and what holds it, the
/// memory and the utilization meter, with a hairline above it unless it is the first.
fn device_row(dv: &Device) -> Element<'_, Msg> {
    let pct = f64::from(dv.util_pct.min(100));
    let state: Element<'_, Msg> = match &dv.allocated_to {
        Some(job) => common::link(
            &format!("running {job}"),
            Msg::Go(Id::LabJobDetail(job.clone())),
        ),
        None => note(if dv.busy { "busy" } else { "idle" }),
    };
    let title = row![
        text(dv.name.clone())
            .font(SANS_SEMIBOLD)
            .size(TYPE_SECTION - 2.0)
            .color(crate::theme::palette().ink),
        ui::badge(&dv.backend, ui::Tone::Signal),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let who = row![note(format!("{} · ", dv.vendor)), state]
        .spacing(0)
        .align_y(Alignment::Center);
    let memory = row![
        ui::sans(
            format!(
                "{} of {} MiB",
                common::count(dv.mem_used_mib),
                common::count(dv.memory_mib)
            ),
            TYPE_META + 0.5,
            crate::theme::palette().ink
        ),
        common::meter(pct / 100.0, false),
        ui::sans(
            common::percent(pct / 100.0),
            TYPE_META + 0.5,
            crate::theme::palette().ink
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .width(Length::Fill);
    column![title, who, memory]
        .spacing(6)
        .padding(Padding {
            top: 10.0,
            right: 0.0,
            bottom: 10.0,
            left: 0.0,
        })
        .width(Length::Fill)
        .into()
}

// ---- Queue -----------------------------------------------------------------

/// The queue: the running jobs first, then the queued ones oldest first, each row opening its job.
fn queue_card(d: &Data) -> Element<'_, Msg> {
    let mut active: Vec<&JobSummary> = d
        .jobs
        .iter()
        .filter(|j| j.status == "queued" || j.status == "running")
        .collect();
    active.sort_by(
        |a, b| match (a.status == "running", b.status == "running") {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.created_ms.cmp(&b.created_ms),
        },
    );
    let running = active.iter().filter(|j| j.status == "running").count();
    let meta = format!(
        "{} queued · {} running",
        common::count((active.len() - running) as u64),
        common::count(running as u64)
    );
    let body: Element<'_, Msg> = if active.is_empty() {
        note(if d.jobs.is_empty() {
            "No jobs yet. Submit one from Jobs to queue a training run."
        } else {
            "Nothing queued or running."
        })
    } else {
        let cols = [
            col("Job", 2, false),
            col("Status", 2, false),
            col("Recipe", 3, false),
            col("Mode", 2, false),
            col("Runs", 1, true),
            col("GPU-h", 3, true),
            col("Created", 3, false),
        ];
        let rows: Vec<Row<'_, Msg>> = active
            .iter()
            .map(|j| Row {
                cells: vec![
                    cell(&j.id, true),
                    ui::badge(&cap(&j.status), status_tone(&j.status)),
                    cell(&j.recipe, true),
                    cell(&j.mode, false),
                    cell(common::count(j.runs), false),
                    cell(
                        format!(
                            "{} / {}",
                            common::number(j.gpu_hours, 2),
                            common::number(j.budget_gpu_hours, 2)
                        ),
                        false,
                    ),
                    cell(common::time(j.created_ms), false),
                ],
                on_press: Some(Msg::Go(Id::LabJobDetail(j.id.clone()))),
            })
            .collect();
        common::table(&cols, rows)
    };
    ui::card_with("Queue", vec![note(meta)], body)
}

// ---- Models and hours ------------------------------------------------------

/// The recent models: the newest first, each row opening its model.
fn models_card(d: &Data) -> Element<'_, Msg> {
    let mut newest: Vec<&ModelView> = d.models.iter().collect();
    newest.sort_by_key(|a| std::cmp::Reverse(a.created_ms));
    newest.truncate(MODELS_SHOWN);
    let body: Element<'_, Msg> = if newest.is_empty() {
        note("No models yet. Import a finished job’s model in Models to see it here.")
    } else {
        let cols = [
            col("Model", 4, false),
            col("Version", 2, false),
            col("State", 2, false),
            col("Params", 2, true),
            col("Size", 2, true),
            col("Created", 3, false),
        ];
        let rows: Vec<Row<'_, Msg>> = newest
            .iter()
            .map(|m| Row {
                cells: vec![
                    name_cell(m),
                    cell(&m.card.version, true),
                    ui::badge(&cap(&m.state), model_tone(&m.state)),
                    cell(
                        if m.parameters > 0 {
                            format!("{:.1} B", m.parameters as f64 / 1e9)
                        } else {
                            "—".to_string()
                        },
                        false,
                    ),
                    cell(common::size(m.size_bytes), false),
                    cell(common::time(m.created_ms), false),
                ],
                on_press: Some(Msg::Go(Id::LabModelDetail(m.id.clone()))),
            })
            .collect();
        common::table(&cols, rows)
    };
    ui::card_with("Recent models", Vec::new(), body)
}

/// A model's name over its id, as the web's model cell.
fn name_cell<'a>(m: &ModelView) -> Element<'a, Msg> {
    column![
        text(m.card.name.clone())
            .font(SANS_SEMIBOLD)
            .size(13.0)
            .color(crate::theme::palette().ink),
        ui::mono(m.id.clone(), 11.0, crate::theme::palette().ink_3),
    ]
    .spacing(2)
    .into()
}

/// The week's training GPU-hours: the figure with the job count, and a bar per local day.
fn hours_card(d: &Data, shown: Option<Vec<f64>>) -> Element<'_, Msg> {
    let values = shown.unwrap_or_else(|| d.week.gpu_hours.clone());
    let total: f64 = values.iter().sum();
    let jobs = d.week.jobs;
    let body = column![
        common::figure(common::number(total, 2)),
        day_bars(&d.week.labels, &values),
        note(format!(
            "{} {} created in the last 7 days",
            common::count(jobs as u64),
            if jobs == 1 { "job" } else { "jobs" }
        )),
    ]
    .spacing(12)
    .width(Length::Fill);
    ui::card_with("Training GPU-hours this week", Vec::new(), body.into())
}

/// One column per day, bottom-aligned, scaled to the tallest, each under its weekday.
fn day_bars<'a>(labels: &[String], values: &[f64]) -> Element<'a, Msg> {
    let max = values.iter().copied().fold(0.0_f64, f64::max);
    let cols: Vec<Element<'a, Msg>> = labels
        .iter()
        .zip(values)
        .map(|(label, &v)| {
            let px = if max > 0.0 {
                ((v.max(0.0) / max * f64::from(BAR_AREA)) as f32).max(BAR_MIN)
            } else {
                BAR_MIN
            };
            let bar = container(Space::new().width(Length::Fill).height(Length::Fixed(px)))
                .width(Length::Fill)
                .style(|_: &Theme| container::Style {
                    background: Some(crate::theme::palette().signal.into()),
                    border: Border {
                        radius: iced::border::Radius {
                            top_left: 4.0,
                            top_right: 4.0,
                            bottom_right: 0.0,
                            bottom_left: 0.0,
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                });
            let area = column![Space::new().height(Length::Fixed(BAR_AREA - px)), bar]
                .width(Length::Fill)
                .height(Length::Fixed(BAR_AREA));
            column![
                area,
                text(label.clone())
                    .font(SANS)
                    .size(11.0)
                    .color(crate::theme::palette().ink_3),
            ]
            .spacing(4)
            .width(Length::Fill)
            .align_x(Alignment::Center)
            .into()
        })
        .collect();
    row(cols)
        .spacing(8)
        .width(Length::Fill)
        .align_y(alignment::Vertical::Bottom)
        .into()
}

// ---- Helpers ---------------------------------------------------------------

fn is_busy(dv: &Device) -> bool {
    dv.allocated_to.is_some() || dv.busy
}

/// A count with its noun, singular for one: `1 GPU`, `2 GPUs`.
fn plural(n: u64, noun: &str) -> String {
    format!(
        "{} {}{}",
        common::count(n),
        noun,
        if n == 1 { "" } else { "s" }
    )
}

/// A status word with its first letter capitalised, as the web's `cap`.
fn cap(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The badge tone of a job's status, as the web's `JOB_TONE`.
fn status_tone(status: &str) -> ui::Tone {
    match status {
        "running" => ui::Tone::Info,
        "done" => ui::Tone::Ok,
        "failed" => ui::Tone::Danger,
        "cancelled" => ui::Tone::Warn,
        _ => ui::Tone::Muted,
    }
}

/// The badge tone of a model's state, as the web's `MODEL_TONE`.
fn model_tone(state: &str) -> ui::Tone {
    match state {
        "trained" => ui::Tone::Info,
        "shadow" => ui::Tone::Signal,
        "promoted" => ui::Tone::Ok,
        _ => ui::Tone::Muted,
    }
}

/// The last seven local days: each day's weekday, its training GPU-hours (by the day a job was
/// created) and the jobs created in the window.
fn week(jobs: &[JobSummary], today: NaiveDate) -> Week {
    let days: Vec<NaiveDate> = (0..WEEK_DAYS)
        .map(|i| {
            today
                .checked_sub_days(Days::new((WEEK_DAYS - 1 - i) as u64))
                .unwrap_or(today)
        })
        .collect();
    let mut week = Week {
        labels: days.iter().map(|d| d.format("%a").to_string()).collect(),
        gpu_hours: vec![0.0; WEEK_DAYS],
        jobs: 0,
    };
    for job in jobs {
        let slot = days.iter().position(|&day| {
            let start = day_start_ms(day);
            let end = day_start_ms(day.succ_opt().unwrap_or(day));
            job.created_ms >= start && job.created_ms < end
        });
        if let Some(i) = slot {
            week.gpu_hours[i] += job.gpu_hours;
            week.jobs += 1;
        }
    }
    week
}

/// Local midnight of `day`, unix milliseconds.
fn day_start_ms(day: NaiveDate) -> u64 {
    day.and_hms_opt(0, 0, 0)
        .and_then(|dt| Local.from_local_datetime(&dt).earliest())
        .map_or(0, |t| t.timestamp_millis().max(0) as u64)
}
