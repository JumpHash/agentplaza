//! The Agents screen (parity with the web console's `agents` view): the roster, filtered by role
//! and status, each row's own actions, and the wizard that spawns agents over it. Under a project
//! it is that project's agents (and the orchestrator), with the bench beside them to add from;
//! under All projects every agent, with where it works and a Put on… picker for the bench.

use std::fmt;

use iced::font::Weight;
use iced::widget::text::Wrapping;
use iced::widget::{Space, Stack, button, column, container, pick_list, row, text, tooltip};
use iced::{Alignment, Background, Border, Color, Element, Font, Length, Task, Theme};
use serde_json::Value;

use super::Id;
use super::project_team;
use super::wizard::{self, WizardMsg};
use crate::console::engine::roles::{agent_hue, role_hue};
use crate::console::ui::Kind;
use crate::console::{Ctx, client, render_card, scope, ui};
use crate::theme::{self};

/// The web's `info` tone on the dark ground.
const INFO: Color = Color::from_rgb8(0x6c, 0xc8, 0xff);
/// The table's head labels and the badges: mono, semibold / medium.
const SANS_MEDIUM: Font = Font {
    weight: Weight::Medium,
    ..theme::SANS
};
const SANS_SEMIBOLD: Font = Font {
    weight: Weight::Semibold,
    ..theme::SANS
};
/// Table cells are 14px serif, as the web's `.table`.
const CELL: f32 = 14.0;
const ACTIONS_WIDTH: f32 = 150.0;

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
    name: String,
    role: String,
    role_label: String,
    orchestrator: bool,
    species: String,
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
        name: field("name"),
        role,
        role_label,
        orchestrator: field("tier") == "orchestrator",
        species: field("species").to_string(),
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
                raw_name(&state.raw, cid),
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
            let line = format!("{} is back on the bench", raw_name(&state.raw, cid));
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

/// The bench beside a project's agents: who could join it, each with Add to project.
fn bench_card<'a>(state: &'a State, project: &str) -> Element<'a, Msg> {
    let waiting: Vec<&Value> = state.raw.iter().filter(|a| scope::on_bench(a)).collect();
    let mut lines: Vec<Element<'a, Msg>> = Vec::new();
    if waiting.is_empty() {
        lines.push(ui::faint("Nobody is on the bench: every agent works on a project.").into());
    } else {
        lines.push(ui::faint("Agents on no project do nothing and sleep. Add one to give it this project's work.").into());
        for a in waiting {
            let cid = a["cid"].as_u64().unwrap_or(0);
            lines.push(project_team::agent_row(
                a,
                Some(ui::btn_sm(
                    "Add to project",
                    Some("plus"),
                    Kind::Secondary,
                    Some(Msg::Join(cid, project.to_string())),
                )),
                None,
            ));
        }
    }
    ui::card("The bench", column(lines).spacing(10).into())
}

/// An agent's name from the roster, for the lines a move shows.
fn raw_name(raw: &[Value], cid: u64) -> String {
    raw.iter()
        .find(|a| a["cid"].as_u64() == Some(cid))
        .map(project_team::agent_name)
        .unwrap_or_else(|| format!("Agent {cid}"))
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

/// The web's `.select`: a transparent field with an ink rule, the handle in ink-2.
fn select_style(_: &Theme, _: pick_list::Status) -> pick_list::Style {
    pick_list::Style {
        text_color: crate::theme::palette().ink,
        placeholder_color: crate::theme::palette().ink_3,
        handle_color: crate::theme::palette().ink_2,
        background: Background::Color(Color::TRANSPARENT),
        border: Border {
            color: crate::theme::palette().ink,
            width: 1.0,
            radius: 0.0.into(),
        },
    }
}

fn list<'a>(state: &'a State, scope: Option<&'a str>) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(e) = &state.error {
        parts.push(ui::error_line(e));
    }
    if let Some(n) = &state.note {
        parts.push(ui::faint(n.clone()).into());
    }
    match &state.agents {
        None => parts.push(ui::loading()),
        Some(all) if all.is_empty() => parts.push(match scope {
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
        }),
        Some(all) => {
            let rows: Vec<&AgentRow> = all
                .iter()
                .filter(|a| {
                    state.role_filter.as_ref().is_none_or(|r| *r == a.role)
                        && state.status_filter.matches(&a.status)
                })
                .collect();
            if rows.is_empty() {
                parts.push(empty(
                    "search",
                    "No agents match",
                    "Clear the filters to see the roster.",
                    Some(ui::btn(
                        "Clear filters",
                        None,
                        Kind::Ghost,
                        Some(Msg::ClearFilters),
                    )),
                ));
            } else {
                parts.push(table(rows, state, scope));
            }
        }
    }
    column(parts).spacing(12).into()
}

/// The roster as the web's table: head labels over an ink rule, a faint rule between rows, the
/// whole framed by a 1px rule.
fn table<'a>(
    rows: Vec<&'a AgentRow>,
    state: &'a State,
    scope: Option<&'a str>,
) -> Element<'a, Msg> {
    // Under All projects, where each agent works (a column taken from Work); under a project, all of them work on it.
    let mut cells = vec![cell(3, head_label("Agent"), false)];
    if scope.is_none() {
        cells.push(cell(3, head_label("Project"), false));
    }
    cells.push(cell(3, head_label("Model"), false));
    cells.push(cell(work_portion(scope), head_label("Work"), false));
    cells.push(cell(1, head_label("Tokens"), true));
    cells.push(
        container(Space::new())
            .width(Length::Fixed(ACTIONS_WIDTH))
            .into(),
    );
    let header = iced::widget::Row::with_children(cells).align_y(Alignment::Center);
    let mut lines: Vec<Element<'a, Msg>> = vec![
        container(header)
            .padding([12, 0])
            .width(Length::Fill)
            .into(),
        rule(crate::theme::palette().ink),
    ];
    let live = scope::picks(&state.projects);
    for (slot, a) in rows.into_iter().enumerate() {
        lines.push(row_view(a, slot, state, scope, &live));
        lines.push(rule(crate::theme::palette().rule_faint));
    }
    container(ui::scroll(column(lines).width(Length::Fill)).width(Length::Fill))
        .style(|_: &Theme| container::Style {
            border: Border {
                color: crate::theme::palette().rule,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .width(Length::Fill)
        .into()
}

fn head_label<'a>(label: &str) -> Element<'a, Msg> {
    text(label.to_string())
        .font(ui::SANS_SEMIBOLD)
        .size(10.5)
        .color(crate::theme::palette().ink_3)
        .wrapping(Wrapping::None)
        .into()
}

/// A table cell at `portion` of the row, its text at the web's 14px inset; `right` aligns it.
fn cell<'a>(portion: u16, content: Element<'a, Msg>, right: bool) -> Element<'a, Msg> {
    container(content)
        .padding([0, 14])
        .width(Length::FillPortion(portion))
        .align_x(if right {
            Alignment::End
        } else {
            Alignment::Start
        })
        // A long prompt ends at its column (the web truncates it), not over the tokens and the actions.
        .clip(true)
        .into()
}

fn row_view<'a>(
    a: &'a AgentRow,
    slot: usize,
    state: &'a State,
    scope: Option<&'a str>,
    live: &[scope::Pick],
) -> Element<'a, Msg> {
    let mut cells = vec![cell(3, agent_cell(a, slot), false)];
    if scope.is_none() {
        cells.push(cell(3, project_cell(a, state, live), false));
    }
    cells.push(cell(3, model_cell(a), false));
    cells.push(cell(work_portion(scope), work_cell(a), false));
    cells.push(cell(1, tokens_cell(a.tokens), true));
    cells.push(
        container(actions(a, scope))
            .padding([0, 14])
            .width(Length::Fixed(ACTIONS_WIDTH))
            .align_x(Alignment::End)
            .into(),
    );
    let line = iced::widget::Row::with_children(cells).align_y(Alignment::Center);
    button(container(line).padding([11, 0]).width(Length::Fill))
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

/// The web's avatar (`avatar(a, 40)`): the agent's robot on its hue, the initial tile under it where the GPU cannot draw.
const AVATAR: f32 = 40.0;

fn agent_cell<'a>(a: &'a AgentRow, slot: usize) -> Element<'a, Msg> {
    let initial = name(a)
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    let tile = container(
        text(initial)
            .font(ui::SANS_SEMIBOLD)
            .size(15.0)
            .color(crate::theme::palette().ink),
    )
    .width(Length::Fixed(AVATAR))
    .height(Length::Fixed(AVATAR))
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .style(|_: &Theme| container::Style {
        background: Some(Background::Color(crate::theme::palette().sheet)),
        border: Border {
            color: crate::theme::palette().ink,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    });
    let spec = render_card::PortraitSpec {
        cid: a.cid,
        orchestrator: a.orchestrator,
        role_hue: if a.orchestrator {
            25.0
        } else {
            role_hue(&a.role)
        },
        body_hue: agent_hue(a.cid),
        offline: matches!(a.status.as_str(), "offline" | "asleep" | "dozing"),
        species: crate::console::engine::robot::species_of(Some(a.species.as_str()), a.cid),
    };
    let layers: Vec<Element<'a, Msg>> = match render_card::robot_portrait(slot, spec, AVATAR) {
        Some(robot) => vec![tile.into(), robot],
        None => vec![tile.into()],
    };
    let portrait = iced::widget::stack(layers)
        .width(Length::Fixed(AVATAR))
        .height(Length::Fixed(AVATAR));
    let name_line = row![
        dot(status_tone(&a.status)),
        text(name(a))
            .font(SANS_SEMIBOLD)
            .size(15.0)
            .color(crate::theme::palette().ink)
            .wrapping(Wrapping::None),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    row![
        portrait,
        column![
            name_line,
            text(a.role_label.clone())
                .font(theme::SANS)
                .size(CELL)
                .color(crate::theme::palette().ink_3)
                .wrapping(Wrapping::None)
        ]
        .spacing(2),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into()
}

/// Where an agent works: its project (to its page), every project (the orchestrator), or the bench with a Put on…
/// picker of the live projects.
fn project_cell<'a>(a: &'a AgentRow, state: &'a State, live: &[scope::Pick]) -> Element<'a, Msg> {
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
    // The bench, and under it the picker that puts the agent on a live project.
    let mut cell = column![badge("Bench", crate::theme::palette().ink_2, false)].spacing(6);
    if !live.is_empty() {
        let cid = a.cid;
        cell = cell.push(
            pick_list(live.to_vec(), None::<scope::Pick>, move |c: scope::Pick| {
                Msg::Join(cid, c.id)
            })
            .placeholder("Put on…")
            .style(select_style)
            .text_size(12.5)
            .font(theme::SANS)
            .padding([3, 8]),
        );
    }
    cell.into()
}

/// The model, and under it where it comes from (its own, its project's, its role's) and the orchestrator's mark: the
/// model ids are long, so the marks get a line of their own rather than being cut off.
fn model_cell<'a>(a: &'a AgentRow) -> Element<'a, Msg> {
    let mut marks = row![].spacing(8).align_y(Alignment::Center);
    if a.orchestrator {
        marks = marks.push(badge("orchestrator", crate::theme::palette().signal, true));
    }
    if a.model_source != "default" {
        let label = match a.model_source.as_str() {
            "agent" => "own",
            "project" => "project",
            _ => "role",
        };
        marks = marks.push(badge(label, crate::theme::palette().ink_2, false));
    }
    let model = text(a.model.clone())
        .font(theme::SANS)
        .size(12.5)
        .color(crate::theme::palette().ink)
        .wrapping(Wrapping::None);
    if a.orchestrator || a.model_source != "default" {
        column![model, marks].spacing(4).into()
    } else {
        model.into()
    }
}

fn work_cell<'a>(a: &'a AgentRow) -> Element<'a, Msg> {
    match &a.work {
        Some((kind, prompt)) => {
            let (color, mark) = work_tone(kind);
            row![
                badge(kind, color, mark),
                text(ui_clip(prompt, 56))
                    .font(theme::SANS)
                    .size(CELL)
                    .color(crate::theme::palette().ink)
                    .wrapping(Wrapping::None),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        }
        None => text("—")
            .font(theme::SANS)
            .size(CELL)
            .color(crate::theme::palette().ink_3)
            .into(),
    }
}

fn tokens_cell<'a>(tokens: u64) -> Element<'a, Msg> {
    text(compact(tokens))
        .font(SANS_MEDIUM)
        .size(12.5)
        .color(crate::theme::palette().ink)
        .into()
}

/// The row's own actions: chat with the agent, cancel its work while it works, under its project send it back to
/// the bench, stop its session.
fn actions<'a>(a: &'a AgentRow, scope: Option<&str>) -> Element<'a, Msg> {
    let cid = a.cid;
    let mut buttons = row![icon_button(
        "conversations",
        "Chat",
        Msg::Go(Id::Agent(cid))
    )]
    .spacing(4);
    if a.status == "working" {
        buttons = buttons.push(icon_button("ban", "Cancel work", Msg::Cancel(cid)));
    }
    if let Some(project) = scope
        && a.project.as_deref() == Some(project)
    {
        buttons = buttons.push(icon_button(
            "x",
            "Back to the bench",
            Msg::Leave(cid, project.to_string()),
        ));
    }
    buttons
        .push(icon_button("stop", "Stop session", Msg::Stop(cid)))
        .align_y(Alignment::Center)
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
            border: Border::default(),
            ..Default::default()
        });
    tooltip(
        b,
        text(tip.to_string()).font(theme::SANS).size(11.0),
        tooltip::Position::Top,
    )
    .into()
}

/// The web's empty state: an icon, a bold mono line, a faint serif sentence, an optional action.
fn empty<'a>(
    icon_name: &str,
    title: &str,
    sentence: &str,
    action: Option<Element<'a, Msg>>,
) -> Element<'a, Msg> {
    let mut body = column![
        ui::icon::<Msg>(icon_name, 16.0, crate::theme::palette().ink_3),
        text(title.to_string())
            .font(Font {
                weight: Weight::Bold,
                ..theme::SANS
            })
            .size(13.0)
            .color(crate::theme::palette().ink),
        text(sentence.to_string())
            .font(theme::SANS)
            .size(14.0)
            .color(crate::theme::palette().ink_3),
    ]
    .spacing(10)
    .align_x(Alignment::Center);
    if let Some(a) = action {
        body = body.push(a);
    }
    container(body)
        .padding([40, 16])
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .into()
}

/// A dot for the status: filled for ok and info (a working agent), outlined otherwise.
fn dot<'a>(tone: Color) -> Element<'a, Msg> {
    container(
        Space::new()
            .width(Length::Fixed(8.0))
            .height(Length::Fixed(8.0)),
    )
    .style(move |_: &Theme| container::Style {
        background: Some(Background::Color(tone)),
        border: Border {
            color: tone,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .into()
}

/// The web's badge: a square marker in the tone (filled, outlined or with a line through it), then
/// the uppercase mono label.
fn badge<'a>(label: &str, color: Color, filled: bool) -> Element<'a, Msg> {
    let mark = container(
        Space::new()
            .width(Length::Fixed(8.0))
            .height(Length::Fixed(8.0)),
    )
    .style(move |_: &Theme| container::Style {
        background: filled.then_some(Background::Color(color)),
        border: Border {
            color,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    });
    row![
        mark,
        text(label.to_string())
            .font(SANS_MEDIUM)
            .size(10.5)
            .color(color)
    ]
    .spacing(7)
    .align_y(Alignment::Center)
    .into()
}

fn status_tone(status: &str) -> Color {
    match status {
        "working" => INFO,
        "idle" => crate::theme::palette().ok,
        _ => crate::theme::palette().ink_3,
    }
}

/// The badge colour and whether its marker is filled, for a work kind.
fn work_tone(kind: &str) -> (Color, bool) {
    match kind {
        "task" => (crate::theme::palette().signal, true),
        "chat" => (INFO, false),
        "motion" => (crate::theme::palette().warn, false),
        _ => (crate::theme::palette().ink_2, false),
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

/// The name an agent chose for itself, or "Unnamed" while it has none.
fn name(a: &AgentRow) -> String {
    if a.name.trim().is_empty() {
        "Unnamed".to_string()
    } else {
        a.name.clone()
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
