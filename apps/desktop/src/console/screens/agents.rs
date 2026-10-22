//! The Agents screen (parity with the web console's `agents` view): the roster, filtered by role
//! and status, each row's own actions, and the wizard that spawns agents over it. Under a project
//! it is that project's agents (and the orchestrator), with the bench beside them to add from;
//! under All projects every agent, with where it works and a Put on… picker for the bench.

use std::fmt;

use iced::font::Weight;
use iced::widget::text::Wrapping;
use iced::widget::{Space, Stack, button, column, container, pick_list, row, text, tooltip};
use iced::{Alignment, Background, Border, Color, Element, Font, Length, Padding, Task, Theme};
use serde_json::Value;

use super::Id;
use super::wizard::{self, WizardMsg};
use crate::console::ui::{Kind, Tone};
use crate::console::{Ctx, client, roles, scope, ui};
use crate::theme::{self};

/// The agent labels and the empty states' titles: sans, medium.
const SANS_MEDIUM: Font = Font {
    weight: Weight::Medium,
    ..theme::SANS
};
/// Table cells are 14px serif, as the web's `.table`.
const CELL: f32 = 14.0;
const ACTIONS_WIDTH: f32 = 220.0;

#[derive(Debug, Default)]
pub struct State {
    pub error: Option<String>,
    /// A line for the last action that went through ("Work cancelled", "Session stopped").
    note: Option<String>,
    /// None until the first load.
    agents: Option<Vec<AgentRow>>,
    /// The roster as the hub sent it: the bench card reads it.
    raw: Vec<Value>,
    /// Every project: the Project column's titles and the Put on… picker's live ones.
    projects: Vec<Value>,
    /// The roles the roster holds, as (id, label), sorted by id.
    roles: Vec<(String, String)>,
    role_filter: Option<String>,
    status_filter: StatusFilter,
    pub wizard: wizard::Wizard,
    /// A load is in flight; hub events that arrive meanwhile only mark `again`.
    loading: bool,
    again: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Result<Value, String>),
    SetRole(Option<String>),
    SetStatus(StatusFilter),
    ClearFilters,
    Cancel(u64),
    Stop(u64),
    Acted(Result<(), String>, &'static str),
    /// Puts a bench agent on a project, or sends a member back to the bench (cid, project).
    Join(u64, String),
    Leave(u64, String),
    /// A move went through (the line to show) or failed.
    Moved(Result<String, String>),
    Wizard(WizardMsg),
    /// A hub event: the roster changed.
    Event(crate::console::events::Event),
}

/// The status filter's choices, as the web's select offers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusFilter {
    #[default]
    All,
    Working,
    Idle,
    Offline,
}

const STATUS_CHOICES: [StatusFilter; 4] = [
    StatusFilter::All,
    StatusFilter::Working,
    StatusFilter::Idle,
    StatusFilter::Offline,
];

impl StatusFilter {
    fn matches(self, status: &str) -> bool {
        match self {
            StatusFilter::All => true,
            StatusFilter::Working => status == "working",
            StatusFilter::Idle => status == "idle",
            // A sleeping (or dozing) agent is not working or idle, so it is filed with the offline ones.
            StatusFilter::Offline => matches!(status, "offline" | "asleep" | "dozing"),
        }
    }
}

impl fmt::Display for StatusFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            StatusFilter::All => "All statuses",
            StatusFilter::Working => "Working",
            StatusFilter::Idle => "Idle",
            StatusFilter::Offline => "Offline",
        })
    }
}

impl From<Id> for Msg {
    fn from(id: Id) -> Msg {
        Msg::Go(id)
    }
}

/// A role choice in the role select: `None` is "All roles".
#[derive(Debug, Clone, PartialEq, Eq)]
struct RoleChoice {
    id: Option<String>,
    label: String,
}

impl fmt::Display for RoleChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

/// One roster row: the fields the table shows, read from `GET /v1/agents`.
#[derive(Debug, Clone)]
struct AgentRow {
    cid: u64,
    role: String,
    role_label: String,
    orchestrator: bool,
    status: String,
    model: String,
    model_source: String,
    tokens: u64,
    /// The work it is doing now: (kind, prompt).
    work: Option<(String, String)>,
    /// The project it works on (None: the bench, or the orchestrator), and why it waits, if it does.
    project: Option<String>,
    held: Option<String>,
    /// On no project, and neither the orchestrator nor a model-only agent.
    bench: bool,
}

fn parse(v: &Value) -> Option<AgentRow> {
    let cid = v.get("cid")?.as_u64()?;
    let field = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    let usage = |key: &str| {
        v.get("usage")
            .and_then(|u| u.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let work = match v.get("work") {
        Some(w) if w.is_object() => Some((
            w.get("kind")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            w.get("prompt")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        )),
        _ => None,
    };
    let role = field("role");
    let role_label = match field("role_label") {
        label if label.is_empty() => role.clone(),
        label => label,
    };
    Some(AgentRow {
        cid,
        role,
        role_label,
        orchestrator: field("tier") == "orchestrator",
        status: field("status"),
        model: field("model"),
        model_source: field("model_source"),
        tokens: usage("input_tokens") + usage("output_tokens"),
        work,
        project: v["project"]
            .as_str()
            .filter(|p| !p.is_empty())
            .map(str::to_string),
        held: v["held"].as_str().map(str::to_string),
        bench: scope::on_bench(v),
    })
}

/// The roles a roster holds: (id, label), sorted by id, each once.
fn roles_of(list: &[AgentRow]) -> Vec<(String, String)> {
    let mut roles: Vec<(String, String)> = list
        .iter()
        .map(|a| (a.role.clone(), a.role_label.clone()))
        .collect();
    roles.sort();
    roles.dedup_by(|a, b| a.0 == b.0);
    roles
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// The hub's events while the screen is open: the web refreshes the roster on `agent` and `work` frames.
pub fn subscription(ctx: &Ctx) -> iced::Subscription<Msg> {
    crate::console::events::subscription(&ctx.client).map(Msg::Event)
}

/// Loads the screen's data (called when it opens, on every poll tick and on a hub event): the roster and the projects.
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    state.loading = true;
    let client = ctx.client.clone();
    client::task(
        move || {
            let agents: Value = client.get("/v1/agents")?;
            let projects = client.get::<Value>("/v1/projects").unwrap_or_default();
            Ok(serde_json::json!({ "agents": agents, "projects": projects }))
        },
        Msg::Loaded,
    )
}

/// A load landed: one more runs if hub events came in while it was in flight.
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
        Msg::Loaded(Ok(v)) => {
            state.raw = v["agents"].as_array().cloned().unwrap_or_default();
            if let Some(projects) = v["projects"].as_array() {
                state.projects = projects.clone();
            }
            // Under a project, its agents and the orchestrator; under All projects, every agent.
            let list: Vec<AgentRow> = state
                .raw
                .iter()
                .filter(|a| scope::in_scope(a, ctx.project.as_deref()))
                .filter_map(parse)
                .collect();
            state.roles = roles_of(&list);
            if state
                .role_filter
                .as_ref()
                .is_some_and(|r| !state.roles.iter().any(|(id, _)| id == r))
            {
                state.role_filter = None;
            }
            state.agents = Some(list);
            state.error = None;
            landed(state, ctx)
        }
        Msg::Loaded(Err(e)) => {
            state.error = Some(e);
            landed(state, ctx)
        }
        Msg::Event(event) => {
            if !matches!(event.kind.as_str(), "agent" | "work") {
                return Task::none();
            }
            if state.loading {
                state.again = true;
                return Task::none();
            }
            load(state, ctx)
        }
        Msg::SetRole(role) => {
            state.role_filter = role;
            Task::none()
        }
        Msg::SetStatus(status) => {
            state.status_filter = status;
            Task::none()
        }
        Msg::ClearFilters => {
            state.role_filter = None;
            state.status_filter = StatusFilter::All;
            Task::none()
        }
        Msg::Cancel(cid) => {
            let client = ctx.client.clone();
            client::task(
                move || {
                    client
                        .call("POST", &format!("/v1/agents/{cid}/cancel"), None)
                        .map(|_| ())
                },
                |r| Msg::Acted(r, "Work cancelled"),
            )
        }
        Msg::Stop(cid) => {
            let client = ctx.client.clone();
            client::task(
                move || {
                    client
                        .call("POST", &format!("/v1/agents/{cid}/stop"), None)
                        .map(|_| ())
                },
                |r| Msg::Acted(r, "Session stopped"),
            )
        }
        Msg::Acted(Ok(()), done) => {
            state.note = Some(done.to_string());
            load(state, ctx)
        }
        Msg::Acted(Err(e), _) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::Join(cid, project) => {
            let client = ctx.client.clone();
            let line = format!(
                "{} works on {} now",
                raw_label(&state.raw, cid),
                scope::title_in(&state.projects, &project)
            );
            client::task(
                move || {
                    client
                        .post::<Value>(
                            &format!("/v1/projects/{project}/members"),
                            &serde_json::json!({ "cid": cid }),
                        )
                        .map(|_| line)
                },
                Msg::Moved,
            )
        }
        Msg::Leave(cid, project) => {
            let client = ctx.client.clone();
            let line = format!("{} is back on the bench", raw_label(&state.raw, cid));
            client::task(
                move || {
                    client
                        .delete(&format!("/v1/projects/{project}/members/{cid}"))
                        .map(|_| line)
                },
                Msg::Moved,
            )
        }
        Msg::Moved(Ok(line)) => {
            state.note = Some(line);
            state.error = None;
            load(state, ctx)
        }
        Msg::Moved(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::Wizard(m) => {
            if let WizardMsg::Done(Err(e)) = &m {
                state.error = Some(e.clone());
            }
            let reload = matches!(m, WizardMsg::Done(Ok(())));
            let step = wizard::update(&mut state.wizard, m, ctx).map(Msg::Wizard);
            if reload {
                Task::batch([step, load(state, ctx)])
            } else {
                step
            }
        }
    }
}

pub fn view<'a>(state: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    let scope = ctx.project.as_deref();
    let mut body = column![list(state, scope)].spacing(16);
    if let Some(project) = scope {
        body = body.push(bench_card(state, project));
    }
    let page = ui::sheet(head(state, scope), body.into());
    if state.wizard.open {
        Stack::new()
            .push(page)
            .push(wizard::view(&state.wizard, ctx).map(Msg::Wizard))
            .into()
    } else {
        page
    }
}

/// Spawns agents through the wizard; under a project, they join it at their first hello.
fn spawn_button<'a>(scope: Option<&str>) -> Element<'a, Msg> {
    ui::btn(
        if scope.is_some() {
            "New agents for this project"
        } else {
            "Spawn agents"
        },
        Some("plus"),
        Kind::Primary,
        Some(Msg::Wizard(WizardMsg::Open)),
    )
}

/// The bench beside a project's agents, as the web's card: who could join it, each with Add to project.
fn bench_card<'a>(state: &'a State, project: &str) -> Element<'a, Msg> {
    let waiting: Vec<AgentRow> = state
        .raw
        .iter()
        .filter(|a| scope::on_bench(a))
        .filter_map(parse)
        .collect();
    let body: Element<'a, Msg> = if waiting.is_empty() {
        ui::faint("Nobody is on the bench: every agent works on a project.").into()
    } else {
        let cols = [
            Col::flex(3, false),
            Col::flex(3, false),
            Col::fixed(ACTIONS_WIDTH, true),
        ];
        let labels = vec![head_cell("Agent"), head_cell("Model"), head_cell("")];
        let n = waiting.len();
        let mut lines: Vec<Element<'a, Msg>> = Vec::new();
        for (slot, a) in waiting.iter().enumerate() {
            lines.push(bench_row(a, project, &cols));
            if slot + 1 < n {
                lines.push(rule(crate::theme::palette().rule));
            }
        }
        column![
            ui::faint(
                "Agents on no project do nothing and sleep. Add one to give it this project's work."
            ),
            head_row(&cols, labels, 0.0),
            column(lines).width(Length::Fill),
        ]
        .spacing(8)
        .into()
    };
    ui::card("The bench", body)
}

/// A bench row: the agent, its model, and Add to project.
fn bench_row<'a>(a: &AgentRow, project: &str, cols: &[Col]) -> Element<'a, Msg> {
    let add = ui::btn_sm::<Msg>(
        "Add to project",
        Some("plus"),
        Kind::Secondary,
        Some(Msg::Join(a.cid, project.to_string())),
    );
    grid_row(cols, vec![agent_cell(a), model_cell(a), add], 0.0)
}

/// An agent's label from the roster, for the lines a move shows.
fn raw_label(raw: &[Value], cid: u64) -> String {
    raw.iter()
        .find(|a| a["cid"].as_u64() == Some(cid))
        .map_or_else(|| roles::agent_label("", cid), roles::agent_label_of)
}

fn head<'a>(state: &'a State, scope: Option<&str>) -> Element<'a, Msg> {
    let mut role_options = vec![RoleChoice {
        id: None,
        label: "All roles".to_string(),
    }];
    role_options.extend(state.roles.iter().map(|(id, label)| RoleChoice {
        id: Some(id.clone()),
        label: label.clone(),
    }));
    let selected_role = role_options
        .iter()
        .find(|c| c.id == state.role_filter)
        .cloned();
    let title = match scope {
        Some(p) => format!("Agents · {}", scope::title_in(&state.projects, p)),
        None => "Agents".to_string(),
    };
    ui::view_head(
        &title,
        vec![
            pick_list(role_options, selected_role, |c: RoleChoice| {
                Msg::SetRole(c.id)
            })
            .style(select_style)
            .text_size(CELL)
            .font(theme::SANS)
            .into(),
            pick_list(
                STATUS_CHOICES.to_vec(),
                Some(state.status_filter),
                Msg::SetStatus,
            )
            .style(select_style)
            .text_size(CELL)
            .font(theme::SANS)
            .into(),
            spawn_button(scope),
        ],
    )
}

/// The web's `.select`: a raised field with a strong rule, the handle in ink-2, the skin's radius.
fn select_style(_: &Theme, _status: pick_list::Status) -> pick_list::Style {
    let p = crate::theme::palette();
    pick_list::Style {
        text_color: p.ink,
        placeholder_color: p.ink_3,
        handle_color: p.ink_2,
        background: Background::Color(p.raised),
        border: Border {
            color: p.rule_strong,
            width: 1.0,
            radius: crate::theme::skin().radius.into(),
        },
    }
}

/// The page's card: the title in the card head, the body flush to the card's edges.
fn flush_card<'a>(title: &str, body: Element<'a, Msg>) -> Element<'a, Msg> {
    let top = container(row![
        ui::card_title(title),
        Space::new().width(Length::Fill)
    ])
    .padding([8, 16])
    .height(Length::Shrink)
    .width(Length::Fill)
    .center_y(Length::Fixed(44.0));
    container(column![top, rule(crate::theme::palette().rule), body])
        .width(Length::Fill)
        .style(ui::card_style)
        .clip(true)
        .into()
}

fn list<'a>(state: &'a State, scope: Option<&'a str>) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(e) = &state.error {
        parts.push(ui::error_line(e));
    }
    if let Some(n) = &state.note {
        parts.push(ui::faint(n.clone()).into());
    }
    let body: Element<'a, Msg> = match &state.agents {
        None => container(ui::loading()).padding(16).into(),
        Some(all) if all.is_empty() => match scope {
            Some(_) => empty(
                "agents",
                "Nobody works on this project yet",
                "Add agents from the bench below, or spawn new ones for it.",
                Some(spawn_button(scope)),
            ),
            None => empty(
                "agents",
                "No agents connected",
                "Spawn agents to fill the roster.",
                Some(spawn_button(scope)),
            ),
        },
        Some(all) => {
            let rows: Vec<&AgentRow> = all
                .iter()
                .filter(|a| {
                    state.role_filter.as_ref().is_none_or(|r| *r == a.role)
                        && state.status_filter.matches(&a.status)
                })
                .collect();
            if rows.is_empty() {
                empty(
                    "search",
                    "No agents match",
                    "Clear the filters to see the roster.",
                    Some(ui::btn(
                        "Clear filters",
                        None,
                        Kind::Ghost,
                        Some(Msg::ClearFilters),
                    )),
                )
            } else {
                table(rows, state, scope)
            }
        }
    };
    parts.push(flush_card("Agents", body));
    column(parts).spacing(12).into()
}

/// One column of a table: its share of the row (or a fixed width), and whether its text sits at the right.
#[derive(Debug, Clone, Copy)]
struct Col {
    portion: u16,
    width: Option<f32>,
    right: bool,
}

impl Col {
    fn flex(portion: u16, right: bool) -> Col {
        Col {
            portion,
            width: None,
            right,
        }
    }

    fn fixed(width: f32, right: bool) -> Col {
        Col {
            portion: 0,
            width: Some(width),
            right,
        }
    }
}

/// The inset of a table cell's text, as the web's `.tbl td`; the first and last cells take `edge` at the card's side.
const CELL_PAD: f32 = 12.0;

/// One table row: its cells in `cols`, each at its share, the outer two inset by `edge`.
fn grid_row<'a>(cols: &[Col], cells: Vec<Element<'a, Msg>>, edge: f32) -> Element<'a, Msg> {
    let last = cols.len().saturating_sub(1);
    let row_cells: Vec<Element<'a, Msg>> = cols
        .iter()
        .zip(cells)
        .enumerate()
        .map(|(i, (col, cell))| {
            let padding = Padding {
                top: 0.0,
                right: if i == last { edge } else { CELL_PAD },
                bottom: 0.0,
                left: if i == 0 { edge } else { CELL_PAD },
            };
            let c = container(cell)
                .padding(padding)
                .clip(true)
                .align_x(if col.right {
                    Alignment::End
                } else {
                    Alignment::Start
                });
            let c = match col.width {
                Some(w) => c.width(Length::Fixed(w)),
                None => c.width(Length::FillPortion(col.portion)),
            };
            c.into()
        })
        .collect();
    iced::widget::Row::with_children(row_cells)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into()
}

/// The table head: the labels in the label voice, over a strong rule (the caller draws it).
fn head_row<'a>(cols: &[Col], head: Vec<Element<'a, Msg>>, edge: f32) -> Element<'a, Msg> {
    container(grid_row(cols, head, edge))
        .padding([8, 0])
        .width(Length::Fill)
        .into()
}

/// A head label: the label voice, or nothing for a column that has no head.
fn head_cell<'a>(label: &str) -> Element<'a, Msg> {
    if label.is_empty() {
        Space::new().into()
    } else {
        ui::label(label).into()
    }
}

/// The roster as the web's table: the head over a strong rule, a rule between rows, inside the card.
fn table<'a>(
    rows: Vec<&'a AgentRow>,
    state: &'a State,
    scope: Option<&'a str>,
) -> Element<'a, Msg> {
    // Under All projects, where each agent works (a column taken from Work); under a project, all of them work on it.
    let mut spec: Vec<(Col, &str)> = vec![(Col::flex(3, false), "Agent")];
    if scope.is_none() {
        spec.push((Col::flex(3, false), "Project"));
    }
    spec.push((Col::flex(3, false), "Model"));
    spec.push((Col::flex(work_portion(scope), false), "Work"));
    spec.push((Col::flex(1, true), "Tokens"));
    spec.push((Col::fixed(ACTIONS_WIDTH, true), ""));
    let cols: Vec<Col> = spec.iter().map(|(c, _)| *c).collect();
    let labels: Vec<Element<'a, Msg>> = spec.iter().map(|(_, l)| head_cell(l)).collect();
    let mut lines: Vec<Element<'a, Msg>> = vec![
        head_row(&cols, labels, EDGE),
        rule(crate::theme::palette().rule_strong),
    ];
    let live = scope::picks(&state.projects);
    let n = rows.len();
    for (slot, a) in rows.into_iter().enumerate() {
        lines.push(row_view(a, state, scope, &cols, &live));
        if slot + 1 < n {
            lines.push(rule(crate::theme::palette().rule));
        }
    }
    ui::scroll(column(lines).width(Length::Fill))
        .width(Length::Fill)
        .into()
}

/// The card's side inset for a flush table (the web's `.card .tbl td:first-child` padding).
const EDGE: f32 = 16.0;

fn row_view<'a>(
    a: &'a AgentRow,
    state: &'a State,
    scope: Option<&'a str>,
    cols: &[Col],
    live: &[scope::Pick],
) -> Element<'a, Msg> {
    let mut cells = vec![agent_cell(a)];
    if scope.is_none() {
        cells.push(project_cell(a, state));
    }
    cells.push(model_cell(a));
    cells.push(work_cell(a));
    cells.push(tokens_cell(a.tokens));
    cells.push(actions(a, scope, live));
    let line = container(grid_row(cols, cells, EDGE))
        .padding([9, 0])
        .width(Length::Fill);
    button(line)
        .on_press(Msg::Go(Id::Agent(a.cid)))
        .style(row_style)
        .padding(0)
        .width(Length::Fill)
        .into()
}

/// The Work column's share of the row: under All projects the Project column takes some of it.
fn work_portion(scope: Option<&str>) -> u16 {
    if scope.is_some() { 4 } else { 3 }
}

/// A clickable row: the hover ground of the web's `tr.clickable`.
fn row_style(_: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: hovered.then_some(Background::Color(crate::theme::palette().hover)),
        text_color: crate::theme::palette().ink,
        border: Border::default(),
        ..Default::default()
    }
}

/// The rule the web draws between table rows and under its head.
fn rule<'a>(color: Color) -> Element<'a, Msg> {
    container(Space::new().height(Length::Fixed(1.0)))
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(color)),
            ..Default::default()
        })
        .into()
}

fn agent_cell<'a>(a: &AgentRow) -> Element<'a, Msg> {
    // The label line: the status dot, the label, and the word for a napping agent (the dot alone does not tell asleep).
    let mut name_line = row![
        ui::dot::<Msg>(status_tone(&a.status)),
        text(roles::agent_label(&a.role, a.cid))
            .font(SANS_MEDIUM)
            .size(CELL)
            .color(crate::theme::palette().ink)
            .wrapping(Wrapping::None),
    ]
    .spacing(7)
    .align_y(Alignment::Center);
    if let Some(word) = sleep_word(&a.status) {
        name_line = name_line.push(ui::badge::<Msg>(word, Tone::Muted));
    }
    column![
        name_line,
        text(a.role_label.clone())
            .font(theme::SANS)
            .size(12.0)
            .color(crate::theme::palette().ink_3)
            .wrapping(Wrapping::None)
    ]
    .spacing(1)
    .into()
}

/// The word a napping status carries: asleep, or dozing as "asleep (light)"; none for the others.
fn sleep_word(status: &str) -> Option<&'static str> {
    match status {
        "asleep" => Some("Asleep"),
        "dozing" => Some("Asleep (light)"),
        _ => None,
    }
}

/// Where an agent works: its project (to its page), every project (the orchestrator), or the bench (a badge; its
/// Put on… picker is in the actions).
fn project_cell<'a>(a: &AgentRow, state: &State) -> Element<'a, Msg> {
    let quiet = |s: &str| {
        text(s.to_string())
            .font(theme::SANS)
            .size(CELL)
            .color(crate::theme::palette().ink_3)
            .wrapping(Wrapping::None)
    };
    if let Some(p) = &a.project {
        let title = scope::title_in(&state.projects, p);
        let link = ui::text_link(&title, Msg::Go(Id::Project(p.clone())));
        return match &a.held {
            Some(why) => tooltip(
                link,
                text(why.clone()).font(theme::SANS).size(11.0),
                tooltip::Position::Top,
            )
            .into(),
            None => link,
        };
    }
    if a.orchestrator {
        return quiet("Every project").into();
    }
    if !a.bench {
        return quiet("—").into();
    }
    ui::badge::<Msg>("Bench", Tone::Muted)
}

/// The model, and under it where it comes from (its own, its project's, its role's) and the orchestrator's mark: the
/// model ids are long, so the marks get a line of their own rather than being cut off.
fn model_cell<'a>(a: &AgentRow) -> Element<'a, Msg> {
    let mut marks = row![].spacing(8).align_y(Alignment::Center);
    if a.orchestrator {
        marks = marks.push(ui::badge::<Msg>("orchestrator", Tone::Signal));
    }
    if a.model_source != "default" {
        let (label, tone) = match a.model_source.as_str() {
            "agent" => ("own", Tone::Info),
            "project" => ("project", Tone::Signal),
            _ => ("role", Tone::Muted),
        };
        marks = marks.push(ui::badge::<Msg>(label, tone));
    }
    let model = ui::mono(a.model.clone(), 12.5, crate::theme::palette().ink);
    if a.orchestrator || a.model_source != "default" {
        column![model, marks].spacing(4).into()
    } else {
        model.into()
    }
}

fn work_cell<'a>(a: &AgentRow) -> Element<'a, Msg> {
    match &a.work {
        Some((kind, prompt)) => row![
            ui::badge::<Msg>(kind, work_tone(kind)),
            text(ui_clip(prompt, 90))
                .font(theme::SANS)
                .size(CELL)
                .color(crate::theme::palette().ink)
                .wrapping(Wrapping::None),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
        None => text("—")
            .font(theme::SANS)
            .size(CELL)
            .color(crate::theme::palette().ink_3)
            .into(),
    }
}

fn tokens_cell<'a>(tokens: u64) -> Element<'a, Msg> {
    text(compact(tokens))
        .font(theme::SANS)
        .size(CELL)
        .color(crate::theme::palette().ink)
        .into()
}

/// The row's own actions, as the web: under its project send it back to the bench; on the bench under All projects,
/// the Put on… picker of the live projects; then chat, cancel its work while it works, and stop its session.
fn actions<'a>(a: &AgentRow, scope: Option<&str>, live: &[scope::Pick]) -> Element<'a, Msg> {
    let cid = a.cid;
    let mut buttons = row![].spacing(4).align_y(Alignment::Center);
    if let Some(project) = scope
        && a.project.as_deref() == Some(project)
    {
        buttons = buttons.push(icon_button(
            "x",
            "Back to the bench",
            Msg::Leave(cid, project.to_string()),
        ));
    }
    if scope.is_none() && a.bench && !live.is_empty() {
        buttons = buttons.push(join_picker(cid, live));
    }
    buttons = buttons.push(icon_button(
        "conversations",
        "Chat",
        Msg::Go(Id::Agent(cid)),
    ));
    if a.status == "working" {
        buttons = buttons.push(icon_button("ban", "Cancel work", Msg::Cancel(cid)));
    }
    buttons
        .push(icon_button("stop", "Stop session", Msg::Stop(cid)))
        .into()
}

/// The Put on… picker that puts bench agent `cid` on a live project.
fn join_picker<'a>(cid: u64, live: &[scope::Pick]) -> Element<'a, Msg> {
    pick_list(live.to_vec(), None::<scope::Pick>, move |c: scope::Pick| {
        Msg::Join(cid, c.id)
    })
    .placeholder("Put on…")
    .style(select_style)
    .text_size(12.5)
    .font(theme::SANS)
    .padding([3, 8])
    .width(Length::Fixed(124.0))
    .into()
}

/// The web's small ghost icon button (28px), with its title as a tooltip.
fn icon_button<'a>(icon_name: &str, tip: &str, on_press: Msg) -> Element<'a, Msg> {
    let glyph = container(ui::icon::<Msg>(
        icon_name,
        13.0,
        crate::theme::palette().ink_2,
    ))
    .center(Length::Fill);
    let b = button(glyph)
        .on_press(on_press)
        .padding(0)
        .width(Length::Fixed(28.0))
        .height(Length::Fixed(28.0))
        .style(|_: &Theme, status: button::Status| button::Style {
            background: matches!(status, button::Status::Hovered | button::Status::Pressed)
                .then_some(Background::Color(crate::theme::palette().hover)),
            text_color: crate::theme::palette().ink,
            border: Border::default().rounded(crate::theme::skin().radius),
            ..Default::default()
        });
    tooltip(
        b,
        text(tip.to_string()).font(theme::SANS).size(11.0),
        tooltip::Position::Top,
    )
    .into()
}

/// The web's empty state: an icon, the title in ink 500, the faint sentence, an optional action; centred.
fn empty<'a>(
    icon_name: &str,
    title: &str,
    sentence: &str,
    action: Option<Element<'a, Msg>>,
) -> Element<'a, Msg> {
    let mut body = column![
        ui::icon::<Msg>(icon_name, 22.0, crate::theme::palette().ink_3),
        text(title.to_string())
            .font(SANS_MEDIUM)
            .size(theme::TYPE_ITEM)
            .color(crate::theme::palette().ink),
        text(sentence.to_string())
            .font(theme::SANS)
            .size(theme::TYPE_ITEM)
            .color(crate::theme::palette().ink_3),
    ]
    .spacing(6)
    .align_x(Alignment::Center);
    if let Some(a) = action {
        body = body.push(a);
    }
    container(body)
        .padding([44, 16])
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .into()
}

/// The status dot's tone: working is info, idle is ok, the rest are muted (offline, asleep, dozing).
fn status_tone(status: &str) -> Color {
    match status {
        "working" => crate::theme::palette().info,
        "idle" => crate::theme::palette().ok,
        _ => crate::theme::palette().ink_3,
    }
}

/// The badge tone for a work kind, as the web draws it.
fn work_tone(kind: &str) -> Tone {
    match kind {
        "task" => Tone::Signal,
        "chat" => Tone::Info,
        _ => Tone::Muted,
    }
}

/// A count in thousands or millions, as the web's `compact`.
fn compact(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        let k = n as f64 / 1_000.0;
        if n < 10_000 {
            format!("{k:.1}k")
        } else {
            format!("{k:.0}k")
        }
    } else {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    }
}

/// Text cut to `max` characters, ending in an ellipsis when cut.
fn ui_clip(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        let head: String = text.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    } else {
        text.to_string()
    }
}
