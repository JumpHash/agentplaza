//! A task's seats, as a panel the task's page holds (the web's `task-seats` card): one row per seat, with its role, its
//! state, the session filling it, the tries and rounds it took, the model it last ran on, and what the step that sent the
//! task back said. The expert view adds the session ids, the recipe, the step and the model policy. The panel reloads on
//! the task's frames and on the frames of its sessions.

use iced::widget::{column, container};
use iced::{Element, Task};

use super::Id;
use super::budget_model::{self as model, policy_text, seat_label, seat_notes};
use super::work_ui::{self, Col, EDGE};
use crate::console::events;
use crate::console::roles;
use crate::console::ui::{self, Tone};
use crate::console::work_api::{self, Seat, SeatState};
use crate::console::{Ctx, client};
use crate::theme;

/// How many characters of a seat's returned findings the table shows.
const RETURNED_CHARS: usize = 160;

#[derive(Debug, Default)]
pub struct Panel {
    task: Option<String>,
    /// The seats as last read; None until the first read answers.
    seats: Option<Vec<Seat>>,
    /// Why the last read failed, or None; a failed reload keeps the rows already shown.
    failure: Option<String>,
    expert: bool,
    /// A read is in flight; a frame that comes meanwhile only asks for one more.
    loading: bool,
    again: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    Go(Id),
    Loaded(Result<Vec<Seat>, String>),
    Expert(bool),
    /// A hub event: a task frame of this task, or a session frame of its seats, reloads the seats.
    Event(events::Event),
}

impl Panel {
    /// The seats of one task.
    pub fn new(task: &str) -> Panel {
        Panel {
            task: Some(task.to_string()),
            ..Default::default()
        }
    }
}

/// Reads the task's seats (called when the page opens and on every poll tick).
pub fn load(panel: &mut Panel, ctx: &Ctx) -> Task<Msg> {
    let Some(task) = panel.task.clone() else {
        return Task::none();
    };
    panel.loading = true;
    let client = ctx.client.clone();
    client::task(move || work_api::seats(&client, &task), Msg::Loaded)
}

/// A read landed: one more runs if frames came in while it was in flight.
fn landed(panel: &mut Panel, ctx: &Ctx) -> Task<Msg> {
    panel.loading = false;
    if std::mem::take(&mut panel.again) {
        load(panel, ctx)
    } else {
        Task::none()
    }
}

pub fn update(panel: &mut Panel, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(Ok(seats)) => {
            panel.seats = Some(seats);
            panel.failure = None;
            landed(panel, ctx)
        }
        Msg::Loaded(Err(message)) => {
            panel.failure = Some(message);
            landed(panel, ctx)
        }
        Msg::Expert(on) => {
            panel.expert = on;
            Task::none()
        }
        Msg::Event(event) => {
            let Some(task) = &panel.task else {
                return Task::none();
            };
            let relevant = match event.kind.as_str() {
                "task" => model::is_task_frame(&event.data, task),
                "session" => model::is_task_session(&event.data, task),
                _ => false,
            };
            if !relevant {
                return Task::none();
            }
            if panel.loading {
                panel.again = true;
                return Task::none();
            }
            load(panel, ctx)
        }
    }
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

pub fn view<'a>(panel: &'a Panel) -> Element<'a, Msg> {
    let actions = vec![work_ui::switch("Expert", panel.expert, Msg::Expert)];
    let body: Element<'a, Msg> = match &panel.seats {
        None => match &panel.failure {
            Some(message) => ui::empty("alert", "The seats could not be loaded", message),
            None => ui::faint("Loading the seats…").into(),
        },
        Some(seats) => {
            let mut lines: Vec<Element<'a, Msg>> = Vec::new();
            if panel.failure.is_some() {
                lines.push(ui::error_line(
                    "Could not refresh the seats. Showing the last ones.",
                ));
            }
            if seats.is_empty() {
                lines.push(ui::empty(
                    "users",
                    "No seats yet",
                    "A task's seats appear once its recipe starts.",
                ));
            } else {
                lines.push(seats_table(panel, seats));
            }
            column(lines).spacing(12).width(iced::Length::Fill).into()
        }
    };
    ui::card_with("Seats", actions, body)
}

/// The seats: the seat's name and role, its state, the session in it (a link), and its details.
fn seats_table<'a>(panel: &'a Panel, seats: &'a [Seat]) -> Element<'a, Msg> {
    let cols = [Col::flex(3), Col::flex(2), Col::flex(2), Col::flex(5)];
    let rows: Vec<Element<'a, Msg>> = seats
        .iter()
        .map(|seat| {
            let cells: Vec<Element<'a, Msg>> = vec![
                column![
                    ui::sans(seat.name.clone(), 14.0, theme::palette().ink),
                    ui::faint(roles::label(&seat.role)),
                ]
                .spacing(2)
                .into(),
                state_badge(seat.state),
                session_cell(panel, seat),
                details(panel, seat),
            ];
            container(work_ui::grid_row(&cols, cells, EDGE))
                .padding([9, 0])
                .width(iced::Length::Fill)
                .into()
        })
        .collect();
    work_ui::table(&cols, &["Seat", "State", "Session", "Details"], rows, EDGE)
}

fn state_badge<'a>(state: SeatState) -> Element<'a, Msg> {
    let tone = match state {
        SeatState::Waiting => Tone::Muted,
        SeatState::Ready => Tone::Info,
        SeatState::Filled => Tone::Signal,
        SeatState::Done => Tone::Ok,
        SeatState::Failed => Tone::Danger,
    };
    ui::badge(seat_label(state), tone)
}

/// The session filling the seat, as a link to its page; nobody yet when no session is in it. The expert view names it.
fn session_cell<'a>(panel: &'a Panel, seat: &'a Seat) -> Element<'a, Msg> {
    match &seat.session {
        Some(id) => ui::text_link(
            if panel.expert { id.as_str() } else { "Open" },
            Msg::Go(Id::Session(id.clone())),
        ),
        None => ui::faint("Nobody yet").into(),
    }
}

/// The seat's tries and rounds, the model it last ran on, the expert line, and what sent the task back.
fn details<'a>(panel: &'a Panel, seat: &'a Seat) -> Element<'a, Msg> {
    let mut lines: Vec<String> = seat_notes(seat);
    if let Some(model) = &seat.last_model {
        lines.push(model.clone());
    }
    if panel.expert {
        lines.push(format!(
            "{} · step {} · {}",
            seat.recipe,
            seat.step,
            policy_text(seat.model_policy)
        ));
    }
    if let Some(returned) = &seat.returned_with {
        lines.push(format!(
            "Returned with: {}",
            ui::ellipsize(returned, RETURNED_CHARS)
        ));
    }
    if lines.is_empty() {
        return ui::faint("—").into();
    }
    column(
        lines
            .into_iter()
            .map(|line| ui::sans(line, 13.0, theme::palette().ink_2).into())
            .collect::<Vec<Element<'a, Msg>>>(),
    )
    .spacing(2)
    .into()
}

#[cfg(test)]
#[path = "task_seats_tests.rs"]
mod tests;
