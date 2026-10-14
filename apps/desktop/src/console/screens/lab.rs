//! The Lab (sidebar `Lab`): the training lab's sections, parity with the web's `#/lab`. This module
//! holds what every Lab screen shares, the section strip ([`frame`]) and [`Section`], and it is the
//! Overview: the GPUs across the fleet (per machine and per device), the queue, the recent models and
//! the week's training GPU-hours with a daily bar chart. The web draws the same four cards in the same
//! order with the same labels and empty states. The other Lab screens are `lab_jobs`,
//! `lab_job_detail`, `lab_recipes`, `lab_datasets`, `lab_models`, `lab_model_detail` and
//! `lab_breeders`; each draws its body inside [`frame`] under its [`Section`].
//!
//! The Overview reloads on the console's 4 s tick and on every `training` line (one load at a time),
//! and its GPU-hours figure and bars settle over a short animation, as the web's count-up does.

use std::time::{Duration, Instant};

use chrono::{Days, Local, NaiveDate, TimeZone};
use iced::font::{self, Font};
use iced::widget::{Space, button, column, container, row, text};
use iced::{
    Alignment, Border, Color, Element, Length, Padding, Subscription, Task, Theme, alignment,
};

use super::Id;
use super::lab_common;
use crate::console::chart;
use crate::console::client::{self, Client};
use crate::console::lab_api::{self, Device, DeviceMachine, JobSummary, ModelView};
use crate::console::{Ctx, ui};
use crate::theme::{self, SANS};

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

/// A Lab screen's frame: the section strip over its body, in the web's padding. A tab sends `on_go`
/// of the screen it opens.
pub fn frame<'a, M: Clone + 'a>(
    current: Section,
    body: Element<'a, M>,
    on_go: impl Fn(Id) -> M + 'a,
) -> Element<'a, M> {
    let page = column![
        lab_tabs(current, move |section: Section| on_go(section.id())),
        body
    ]
    .spacing(22)
    .height(Length::Fill);
    container(page)
        .padding(Padding {
            top: 20.0,
            right: 28.0,
            bottom: 28.0,
            left: 28.0,
        })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

// ---- Overview --------------------------------------------------------------

const MIB: u64 = 1024 * 1024;
/// The GPU-hours window: the last seven local days, today included.
const WEEK_DAYS: usize = 7;
const MODELS_SHOWN: usize = 6;
const MACHINES_PER_ROW: usize = 3;
/// The height of the day bars' area, in pixels; the tallest day fills it.
const BAR_AREA: f32 = 120.0;
/// How long the GPU-hours figure and bars take to settle after a load.
const ANIM: Duration = Duration::from_millis(600);
const VALUE: Font = Font {
    weight: font::Weight::Bold,
    ..theme::SANS
};
const ITALIC: Font = Font {
    style: font::Style::Italic,
    ..theme::SANS
};
const SANS_SEMIBOLD: Font = ui::SANS_SEMIBOLD;

/// The last seven days, from the fetch: the day labels (`MM-DD`), each day's training GPU-hours and
/// the number of jobs created in the window.
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
    let live = lab_common::live(&ctx.client)
        .filter_map(|e| lab_common::training_frame(&e))
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
        parts.push(ui::error_line(err));
    }
    match &state.data {
        None => {
            for title in [
                "GPUs across the fleet",
                "Queue",
                "Recent models",
                "Training GPU-hours this week",
            ] {
                parts.push(lab_card(
                    title,
                    Vec::new(),
                    "",
                    ui::faint("Reading the lab…").into(),
                ));
            }
        }
        Some(d) => {
            parts.push(fleet_card(d));
            parts.push(queue_card(d));
            parts.push(models_card(d));
            parts.push(hours_card(d, state.gpu_hours.as_ref().map(|f| f.at(now))));
        }
    }
    frame(
        Section::Overview,
        ui::scroll(column(parts).spacing(22).width(Length::Fill))
            .height(Length::Fill)
            .into(),
        Msg::Go,
    )
}

// ---- Strip and links -------------------------------------------------------

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
    column![
        strip,
        container(Space::new().height(1.0))
            .width(Length::Fill)
            .style(|_: &Theme| container::Style {
                background: Some(crate::theme::palette().rule.into()),
                ..Default::default()
            })
    ]
    .width(Length::Fill)
    .into()
}

/// A link in the Lab, as the web's `.lab-overview-link`: the accent ink, ink on the pointer. The mono
/// face for ids, the body's serif for words.
fn link<'a>(label: &str, msg: Msg, mono: bool) -> Element<'a, Msg> {
    let body = if mono {
        text(label.to_string()).font(SANS).size(12.0)
    } else {
        text(label.to_string()).font(theme::SANS).size(14.0)
    };
    button(body)
        .padding(0.0)
        .style(|_: &Theme, status: button::Status| button::Style {
            background: None,
            text_color: if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                crate::theme::palette().ink
            } else {
                crate::theme::palette().signal_ink
            },
            border: Border::default(),
            ..Default::default()
        })
        .on_press(msg)
        .into()
}

// ---- Cards -----------------------------------------------------------------

/// A ruled card as the web's `.lab-card`: a head (the title on the left, tags and a muted meta on the
/// right) over a rule, then the body padded 16.
fn lab_card<'a>(
    title: &str,
    tags: Vec<Element<'a, Msg>>,
    meta: &str,
    body: Element<'a, Msg>,
) -> Element<'a, Msg> {
    let mut line = row![
        text(title.to_string())
            .font(SANS_SEMIBOLD)
            .size(12.0)
            .color(crate::theme::palette().ink),
        Space::new().width(Length::Fill)
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    for tag in tags {
        line = line.push(tag);
    }
    if !meta.is_empty() {
        line = line.push(ui::faint(meta.to_string()).size(12.0));
    }
    let head = container(line)
        .padding(Padding {
            top: 12.0,
            right: 16.0,
            bottom: 10.0,
            left: 16.0,
        })
        .width(Length::Fill);
    card_shell(head.into(), body)
}

/// The frame of a card: its head over a rule, then the body padded 16.
fn card_shell<'a>(head: Element<'a, Msg>, body: Element<'a, Msg>) -> Element<'a, Msg> {
    let rule = container(Space::new().height(1.0))
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().rule.into()),
            ..Default::default()
        });
    container(column![
        head,
        rule,
        container(body).padding(16).width(Length::Fill)
    ])
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

/// The GPUs across the fleet: a card per machine, in rows of three.
fn fleet_card(d: &Data) -> Element<'_, Msg> {
    let devices: usize = d.machines.iter().map(|m| m.devices.len()).sum();
    let busy: usize = d
        .machines
        .iter()
        .flat_map(|m| m.devices.iter())
        .filter(|dv| is_busy(dv))
        .count();
    let meta = format!(
        "{} · {} · {} busy",
        plural(d.machines.len() as u64, "machine"),
        plural(devices as u64, "GPU"),
        lab_common::count(busy as u64)
    );
    let body: Element<'_, Msg> = if devices == 0 {
        ui::empty(
            "machine",
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
                row(cards).spacing(16).width(Length::Fill).into()
            })
            .collect();
        column(rows).spacing(16).width(Length::Fill).into()
    };
    lab_card("GPUs across the fleet", Vec::new(), &meta, body)
}

/// One machine, as the web's card: its id with its state and trainer on the head's first line, and
/// its meta on the second (the web's head wraps the same way). An offline machine's text is dim.
fn machine_card(m: &DeviceMachine) -> Element<'_, Msg> {
    let dim = !m.online;
    let busy = m.devices.iter().filter(|dv| is_busy(dv)).count();
    let title_line = row![
        text(m.id.clone())
            .font(ui::MONO_SEMIBOLD)
            .size(12.0)
            .color(ink(dim)),
        Space::new().width(Length::Fill),
        ui::badge(
            if m.online { "online" } else { "offline" },
            if m.online {
                ui::Tone::Ok
            } else {
                ui::Tone::Muted
            }
        ),
        ui::badge(
            if m.trainer_ok {
                "trainer up"
            } else {
                "trainer down"
            },
            if m.trainer_ok {
                ui::Tone::Info
            } else {
                ui::Tone::Warn
            }
        ),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let meta = ui::faint(format!(
        "{} · {} busy",
        plural(m.devices.len() as u64, "GPU"),
        lab_common::count(busy as u64)
    ))
    .size(12.0);
    let head = container(column![title_line, meta].spacing(4))
        .padding(Padding {
            top: 12.0,
            right: 16.0,
            bottom: 10.0,
            left: 16.0,
        })
        .width(Length::Fill);
    let body: Element<'_, Msg> = if m.devices.is_empty() {
        ui::faint("No devices reported.").into()
    } else {
        let mut list = column![].spacing(14).width(Length::Fill);
        for (i, dv) in m.devices.iter().enumerate() {
            if i > 0 {
                list = list.push(ui::hairline());
            }
            list = list.push(device_row(dv, dim));
        }
        list.into()
    };
    card_shell(head.into(), body)
}

/// One device: its name, vendor and backend, its memory, the utilization bar and what runs on it.
fn device_row(dv: &Device, dim: bool) -> Element<'_, Msg> {
    let pct = f64::from(dv.util_pct.min(100));
    let memory = format!(
        "{} of {} memory used",
        lab_common::size(dv.mem_used_mib * MIB),
        lab_common::size(dv.memory_mib * MIB)
    );
    let run: Element<'_, Msg> = match &dv.allocated_to {
        Some(job) => link(
            &format!("running {job}"),
            Msg::Go(Id::LabJobDetail(job.clone())),
            true,
        ),
        None => ui::faint(if dv.busy { "busy" } else { "idle" })
            .size(12.0)
            .into(),
    };
    let name = row![
        text(dv.name.clone())
            .font(SANS_SEMIBOLD)
            .size(13.0)
            .color(ink(dim)),
        ui::faint(dv.vendor.clone()).size(12.0),
        ui::badge(&dv.backend, ui::Tone::Info),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let util = row![
        util_bar(pct),
        text(lab_common::percent(pct / 100.0))
            .font(SANS)
            .size(12.0)
            .color(ink(dim))
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .width(Length::Fill);
    column![
        name,
        text(memory).font(SANS).size(12.0).color(ink_2(dim)),
        util,
        run
    ]
    .spacing(6)
    .width(Length::Fill)
    .into()
}

/// The text colour of a line: ink, or ink-3 for a dim (offline) one.
fn ink(dim: bool) -> Color {
    if dim {
        crate::theme::palette().ink_3
    } else {
        crate::theme::palette().ink
    }
}

fn ink_2(dim: bool) -> Color {
    if dim {
        crate::theme::palette().ink_3
    } else {
        crate::theme::palette().ink_2
    }
}

/// The utilization meter: a hairline track with the share in the accent colour.
fn util_bar<'a>(pct: f64) -> Element<'a, Msg> {
    let on = pct.round().clamp(0.0, 100.0) as u16;
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if on > 0 {
        parts.push(
            container(Space::new().height(Length::Fill))
                .width(Length::FillPortion(on))
                .height(Length::Fill)
                .style(|_: &Theme| container::Style {
                    background: Some(crate::theme::palette().signal.into()),
                    ..Default::default()
                })
                .into(),
        );
    }
    if on < 100 {
        parts.push(
            Space::new()
                .width(Length::FillPortion(100 - on))
                .height(Length::Fill)
                .into(),
        );
    }
    container(row(parts).width(Length::Fill).height(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fixed(8.0))
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

/// The queue: the running jobs first, then the queued ones oldest first, each linked to its detail.
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
        lab_common::count((active.len() - running) as u64),
        lab_common::count(running as u64)
    );
    let body: Element<'_, Msg> = if active.is_empty() {
        if d.jobs.is_empty() {
            ui::empty(
                "tasks",
                "No jobs yet",
                "Submit one from Jobs to queue a training run.",
            )
        } else {
            ui::empty(
                "tasks",
                "Nothing queued",
                "Every job has finished. Queued and running jobs show here.",
            )
        }
    } else {
        let cols = [
            col("Job", 3, false),
            col("Status", 2, false),
            col("Recipe", 4, false),
            col("Mode", 1, false),
            col("Runs", 1, true),
            col("GPU-h used / budget", 3, true),
            col("Created", 3, false),
        ];
        let rows: Vec<Vec<Element<'_, Msg>>> = active
            .iter()
            .map(|j| {
                vec![
                    link(&j.id, Msg::Go(Id::LabJobDetail(j.id.clone())), true),
                    ui::badge(&j.status, status_tone(&j.status)),
                    cell(&j.recipe, true),
                    cell(&j.mode, false),
                    cell(&lab_common::count(j.runs), false),
                    cell(
                        &format!(
                            "{} / {}",
                            lab_common::number(j.gpu_hours, 2),
                            lab_common::number(j.budget_gpu_hours, 2)
                        ),
                        false,
                    ),
                    cell(&lab_common::time(j.created_ms), false),
                ]
            })
            .collect();
        table(&cols, rows)
    };
    lab_card("Queue", Vec::new(), &meta, body)
}

/// The recent models: the newest first, each linked to its detail.
fn models_card(d: &Data) -> Element<'_, Msg> {
    let mut newest: Vec<&ModelView> = d.models.iter().collect();
    newest.sort_by(|a, b| b.created_ms.cmp(&a.created_ms));
    newest.truncate(MODELS_SHOWN);
    let meta = format!("{} registered", lab_common::count(d.models.len() as u64));
    let body: Element<'_, Msg> = if newest.is_empty() {
        ui::empty(
            "models",
            "No models yet",
            "Import a finished job's model in Models to see it here.",
        )
    } else {
        let cols = [
            col("Model", 4, false),
            col("State", 2, false),
            col("Parameters", 2, true),
            col("Size", 2, true),
            col("Created", 3, false),
        ];
        let rows: Vec<Vec<Element<'_, Msg>>> = newest
            .iter()
            .map(|m| {
                let name = if m.card.version.is_empty() {
                    m.card.name.clone()
                } else {
                    format!("{} {}", m.card.name, m.card.version)
                };
                vec![
                    link(&name, Msg::Go(Id::LabModelDetail(m.id.clone())), false),
                    ui::badge(&m.state, model_tone(&m.state)),
                    cell(&lab_common::count(m.parameters), true),
                    cell(&lab_common::size(m.size_bytes), true),
                    cell(&lab_common::time(m.created_ms), false),
                ]
            })
            .collect();
        table(&cols, rows)
    };
    lab_card("Recent models", Vec::new(), &meta, body)
}

/// The week's training GPU-hours: the figure with the job count, and a bar per local day.
fn hours_card(d: &Data, shown: Option<Vec<f64>>) -> Element<'_, Msg> {
    let values = shown.unwrap_or_else(|| d.week.gpu_hours.clone());
    let total: f64 = values.iter().sum();
    let jobs = d.week.jobs;
    let stat = column![
        ui::label("GPU-hours, last 7 days"),
        text(lab_common::gpu_hours(total))
            .font(VALUE)
            .size(30.0)
            .color(crate::theme::palette().ink),
        text(format!(
            "{} {} created in the last 7 days",
            lab_common::count(jobs as u64),
            if jobs == 1 { "job" } else { "jobs" }
        ))
        .font(ITALIC)
        .size(13.0)
        .color(crate::theme::palette().ink_2),
    ]
    .spacing(4);
    let stat = container(stat)
        .padding(Padding {
            top: 12.0,
            right: 16.0,
            bottom: 12.0,
            left: 16.0,
        })
        .width(Length::Fixed(280.0))
        .style(|_: &Theme| container::Style {
            border: Border {
                color: crate::theme::palette().rule,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        });
    let body = column![stat, day_bars(&d.week.labels, &values)]
        .spacing(16)
        .width(Length::Fill);
    lab_card("Training GPU-hours this week", Vec::new(), "", body.into())
}

/// One column per day, bottom-aligned, scaled to the tallest; each carries its value and its label.
fn day_bars<'a>(labels: &[String], values: &[f64]) -> Element<'a, Msg> {
    let max = values.iter().copied().fold(0.0_f64, f64::max);
    let cols: Vec<Element<'a, Msg>> = labels
        .iter()
        .zip(values)
        .map(|(label, &v)| {
            let px = if max > 0.0 {
                (v.max(0.0) / max * f64::from(BAR_AREA)) as f32
            } else {
                0.0
            };
            let bar = container(Space::new().width(Length::Fill).height(Length::Fixed(px)))
                .width(Length::Fill)
                .style(|_: &Theme| container::Style {
                    background: Some(crate::theme::palette().signal.into()),
                    ..Default::default()
                });
            let area = column![Space::new().height(Length::Fixed(BAR_AREA - px)), bar]
                .width(Length::Fill)
                .height(Length::Fixed(BAR_AREA));
            column![
                text(lab_common::number(v, 2))
                    .font(SANS)
                    .size(11.0)
                    .color(crate::theme::palette().ink_2),
                area,
                text(label.clone())
                    .font(SANS)
                    .size(10.5)
                    .color(crate::theme::palette().ink_3),
            ]
            .spacing(6)
            .width(Length::Fill)
            .align_x(Alignment::Center)
            .into()
        })
        .collect();
    row(cols).spacing(10).width(Length::Fill).into()
}

// ---- Tables ----------------------------------------------------------------

/// A table column: its head, its share of the width, and whether its cells read right-aligned.
struct Col {
    head: &'static str,
    portion: u16,
    right: bool,
}

fn col(head: &'static str, portion: u16, right: bool) -> Col {
    Col {
        head,
        portion,
        right,
    }
}

/// A table: mono caps heads over a rule, then the rows, each cell in its column's share of the width.
fn table<'a>(cols: &[Col], rows: Vec<Vec<Element<'a, Msg>>>) -> Element<'a, Msg> {
    let heads: Vec<Element<'a, Msg>> = cols
        .iter()
        .map(|c| {
            let label = text(c.head.to_string())
                .font(SANS)
                .size(10.5)
                .color(crate::theme::palette().ink_3);
            aligned(label.into(), c)
        })
        .collect();
    let mut body = column![row(heads).spacing(12).width(Length::Fill), ui::hairline()]
        .spacing(6)
        .width(Length::Fill);
    for cells in rows {
        let line: Vec<Element<'a, Msg>> = cells
            .into_iter()
            .zip(cols)
            .map(|(cell, c)| aligned(cell, c))
            .collect();
        body = body.push(
            row(line)
                .spacing(12)
                .align_y(Alignment::Center)
                .width(Length::Fill),
        );
        body = body.push(
            container(Space::new().height(1.0))
                .width(Length::Fill)
                .style(|_: &Theme| container::Style {
                    background: Some(crate::theme::palette().rule_faint.into()),
                    ..Default::default()
                }),
        );
    }
    body.into()
}

fn aligned<'a>(content: Element<'a, Msg>, c: &Col) -> Element<'a, Msg> {
    let align = if c.right {
        alignment::Horizontal::Right
    } else {
        alignment::Horizontal::Left
    };
    container(content)
        .width(Length::FillPortion(c.portion))
        .align_x(align)
        .into()
}

/// A table cell in ink: mono for ids and recipes, the body's serif for the rest (as the web's `td`).
fn cell<'a>(value: &str, mono: bool) -> Element<'a, Msg> {
    let face = if mono { SANS } else { theme::SANS };
    text(value.to_string())
        .font(face)
        .size(13.0)
        .color(crate::theme::palette().ink)
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
        lab_common::count(n),
        noun,
        if n == 1 { "" } else { "s" }
    )
}

/// The badge tone of a job's status, as the web's `JOB_TONE`.
fn status_tone(status: &str) -> ui::Tone {
    match status {
        "running" => ui::Tone::Signal,
        "done" => ui::Tone::Ok,
        "failed" => ui::Tone::Danger,
        "queued" => ui::Tone::Info,
        _ => ui::Tone::Muted,
    }
}

/// The badge tone of a model's state, as the web's `MODEL_TONE`.
fn model_tone(state: &str) -> ui::Tone {
    match state {
        "trained" => ui::Tone::Info,
        "shadow" => ui::Tone::Warn,
        "promoted" => ui::Tone::Ok,
        _ => ui::Tone::Muted,
    }
}

/// The last seven local days: each day's label, its training GPU-hours (by the day a job was created)
/// and the jobs created in the window.
fn week(jobs: &[JobSummary], today: NaiveDate) -> Week {
    let days: Vec<NaiveDate> = (0..WEEK_DAYS)
        .map(|i| {
            today
                .checked_sub_days(Days::new((WEEK_DAYS - 1 - i) as u64))
                .unwrap_or(today)
        })
        .collect();
    let mut week = Week {
        labels: days.iter().map(|d| d.format("%m-%d").to_string()).collect(),
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
