//! The Sessions screen (the web's `sessions` view): the sessions of the project in scope, or of every project under All
//! projects, live ones first and then the ended ones, newest first within each group. A row is a session: its state
//! (and why it waits), its seat, its role, its task and project (links), its model, its usage and its start; a row opens
//! the session. The expert view adds the node, the VM, the run, the engine, the attempt, the compactions and the id. An
//! agent is a session's VM now, so an agent's old link opens the session on that VM (see `session.rs`).

use iced::widget::{column, container, text};
use iced::{Element, Length, Task};

use super::Id;
use super::session_model::{self as model, PLAZA_PROJECT};
use super::work_ui::{self, Col, EDGE};
use crate::console::work_api::{self, Session};
use crate::console::{Ctx, client, roles, scope, ui};
use crate::theme;

/// The rows drawn at first; "Show more" draws the rest.
const LIST_ROWS: usize = 200;
/// The size of a table cell's words, as the web's `.table`.
const CELL: f32 = 14.0;

#[derive(Debug, Default)]
pub struct State {
    /// The sessions as the hub last answered; None until the first answer.
    sessions: Option<Vec<Session>>,
    /// Why the last load failed, or None.
    failure: Option<String>,
    live_only: bool,
    expert: bool,
    /// Every row is drawn, not the first LIST_ROWS (set by "Show more").
    show_all: bool,
    /// A load is in flight; a session frame that comes meanwhile only asks for one more.
    loading: bool,
    again: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Result<Vec<Session>, String>),
    LiveOnly(bool),
    Expert(bool),
    MoreRows,
    /// A hub event: a session frame reloads the list.
    Event(crate::console::events::Event),
}

pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// The hub's events while the screen is open: the session frames reload the list.
pub fn subscription(ctx: &Ctx) -> iced::Subscription<Msg> {
    crate::console::events::subscription(&ctx.client).map(Msg::Event)
}

/// Loads the sessions of the project in scope (called when the screen opens and on every poll tick).
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    state.loading = true;
    let client = ctx.client.clone();
    let project = ctx.project.clone();
    client::task(
        move || work_api::sessions(&client, project.as_deref(), None, false),
        Msg::Loaded,
    )
}

/// A load landed: one more runs if session frames came in while it was in flight.
fn landed(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    state.loading = false;
    if std::mem::take(&mut state.again) {
        load(state, ctx)
    } else {
        Task::none()
    }
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(Ok(list)) => {
            state.sessions = Some(list);
            state.failure = None;
            landed(state, ctx)
        }
        Msg::Loaded(Err(e)) => {
            state.failure = Some(e);
            landed(state, ctx)
        }
        Msg::LiveOnly(on) => {
            state.live_only = on;
            state.show_all = false;
            Task::none()
        }
        Msg::Expert(on) => {
            state.expert = on;
            Task::none()
        }
        Msg::MoreRows => {
            state.show_all = true;
            Task::none()
        }
        Msg::Event(event) => {
            if event.kind != "session" {
                return Task::none();
            }
            if state.loading {
                state.again = true;
                return Task::none();
            }
            load(state, ctx)
        }
    }
}

pub fn view<'a>(state: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    let project = ctx.project.as_deref();
    let intro = match project {
        Some(project) => format!(
            "{}'s sessions: its tasks' steps, and its planner.",
            scope::title(project)
        ),
        None => "Every project's sessions: a task's steps, each planner, and the orchestrator."
            .to_string(),
    };
    let head = ui::view_head_with_intro(
        "Sessions",
        &intro,
        vec![
            work_ui::switch("Live only", state.live_only, Msg::LiveOnly),
            work_ui::switch("Expert", state.expert, Msg::Expert),
        ],
    );
    let body = match &state.sessions {
        None => match &state.failure {
            Some(error) => ui::empty("alert", "Could not load the sessions", error),
            None => ui::loading(),
        },
        Some(list) => {
            let mut lines: Vec<Element<'a, Msg>> = Vec::new();
            if let Some(error) = &state.failure {
                lines.push(ui::error_line(&format!(
                    "Could not refresh the sessions: {error}"
                )));
            }
            lines.push(content(state, list, project));
            column(lines).spacing(12).into()
        }
    };
    ui::sheet(head, body)
}

/// The sessions table, or the empty state that says why there is none.
fn content<'a>(state: &'a State, list: &'a [Session], project: Option<&str>) -> Element<'a, Msg> {
    let rows = model::rows(list, project, state.live_only);
    if rows.is_empty() {
        let any_in_scope = list.iter().any(|s| model::in_scope(s, project));
        return if any_in_scope {
            ui::empty(
                "agents",
                "Nothing is live",
                "Turn off Live only to see the sessions that have ended.",
            )
        } else if project.is_some() {
            ui::empty(
                "agents",
                "No sessions yet",
                "This project has no sessions yet. Its tasks run as sessions, one step each, and its planner runs as one.",
            )
        } else {
            ui::empty(
                "agents",
                "No sessions yet",
                "The plaza has no sessions yet. Each step of a task, each project planner and the orchestrator run as a session.",
            )
        };
    }
    let shown = if state.show_all {
        rows.len()
    } else {
        rows.len().min(LIST_ROWS)
    };
    let mut lines: Vec<Element<'a, Msg>> = vec![work_ui::table(
        &columns(state.expert),
        &labels(state.expert),
        rows[..shown]
            .iter()
            .map(|s| session_row(s, state.expert))
            .collect(),
        EDGE,
    )];
    if rows.len() > shown {
        lines.push(ui::btn(
            &format!("Show {} more", rows.len() - shown),
            None,
            ui::Kind::Ghost,
            Some(Msg::MoreRows),
        ));
    }
    container(column(lines).spacing(12))
        .style(ui::card_style)
        .width(Length::Fill)
        .clip(true)
        .padding(iced::Padding {
            top: 0.0,
            right: 0.0,
            bottom: 8.0,
            left: 0.0,
        })
        .into()
}

/// The columns: their shares of the width, the expert ones after the plain ones.
fn columns(expert: bool) -> Vec<Col> {
    let mut cols = vec![
        Col::flex(3),
        Col::flex(2),
        Col::flex(2),
        Col::flex(2),
        Col::flex(2),
        Col::flex(2),
        Col::right(2),
        Col::right(1),
    ];
    if expert {
        cols.extend([
            Col::flex(2),
            Col::right(1),
            Col::flex(2),
            Col::flex(2),
            Col::right(1),
            Col::right(1),
            Col::flex(2),
        ]);
    }
    cols
}

fn labels(expert: bool) -> Vec<&'static str> {
    let mut labels = vec![
        "State", "Seat", "Role", "Task", "Project", "Model", "Usage", "Started",
    ];
    if expert {
        labels.extend([
            "Node",
            "VM",
            "Run",
            "Engine",
            "Attempt",
            "Compactions",
            "Id",
        ]);
    }
    labels
}

/// One session's row, a button that opens it.
fn session_row<'a>(s: &Session, expert: bool) -> Element<'a, Msg> {
    let mut cells: Vec<Element<'a, Msg>> = vec![
        state_cell(s),
        plain(model::seat_text(s)),
        plain(roles::label(&s.role)),
        task_cell(s),
        project_cell(s),
        model_cell(s),
        usage_cell(s),
        ui::faint(ui::ago(s.started_ms.unwrap_or(s.created_ms))).into(),
    ];
    if expert {
        cells.extend([
            plain(s.node.clone().unwrap_or_else(|| "—".into())),
            plain(s.cid.map_or_else(|| "—".into(), |cid| cid.to_string())),
            plain(s.run.clone().unwrap_or_else(|| "—".into())),
            plain(s.engine.clone()),
            plain(s.attempt.to_string()),
            plain(s.compactions.to_string()),
            ui::mono(s.id.clone(), 12.0, theme::palette().ink_2).into(),
        ]);
    }
    let line = container(work_ui::grid_row(&columns(expert), cells, EDGE))
        .padding([9, 0])
        .width(Length::Fill);
    work_ui::clickable(line.into(), Msg::Go(Id::Session(s.id.clone())))
}

/// A word in the table's ink.
fn plain<'a>(words: String) -> Element<'a, Msg> {
    text(words).size(CELL).color(theme::palette().ink).into()
}

/// The state's badge, and beneath it why the session waits, when it does.
fn state_cell<'a>(s: &Session) -> Element<'a, Msg> {
    let badge = ui::badge(model::state_label(s.state), model::state_tone(s.state));
    match model::paused_text(s.paused_by) {
        Some(reason) => column![badge, ui::faint(reason.to_string())]
            .spacing(3)
            .into(),
        None => badge,
    }
}

/// The task, as a link to its page; a session with no task says so.
fn task_cell<'a>(s: &Session) -> Element<'a, Msg> {
    match &s.task {
        Some(task) => ui::text_link(task, Msg::Go(Id::Task(task.clone()))),
        None => ui::faint("no task").into(),
    }
}

/// The project, as a link to its page; the orchestrator works for the whole plaza.
fn project_cell<'a>(s: &Session) -> Element<'a, Msg> {
    if s.project == PLAZA_PROJECT {
        return plain("The plaza".into());
    }
    ui::text_link(
        &scope::title(&s.project),
        Msg::Go(Id::Project(s.project.clone())),
    )
}

fn model_cell<'a>(s: &Session) -> Element<'a, Msg> {
    match &s.model {
        Some(model) => ui::mono(model.clone(), 13.0, theme::palette().ink).into(),
        None => ui::faint("—").into(),
    }
}

/// The usage's tokens in and out, and its cost beneath.
fn usage_cell<'a>(s: &Session) -> Element<'a, Msg> {
    column![
        plain(model::usage_text(&s.usage)),
        ui::faint(model::cost_text(s.usage.cost_micros)),
    ]
    .align_x(iced::Alignment::End)
    .into()
}

#[cfg(test)]
#[path = "sessions_tests.rs"]
mod tests;
