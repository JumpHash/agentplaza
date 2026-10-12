//! The Agents screen (parity with the web console's `agents` view): the roster, filtered by role
//! and status, each row's own actions, and the wizard that spawns agents over it.

use std::fmt;

use iced::widget::{column, container, mouse_area, pick_list, row, scrollable, Space, Stack};
use iced::{Alignment, Border, Element, Length, Task, Theme};
use serde_json::Value;

use super::wizard::{self, WizardMsg};
use super::Id;
use crate::console::{client, ui, Ctx};
use crate::console::ui::{Kind, Tone, INK, INK_2, INK_3, RULE};

#[derive(Debug, Default)]
pub struct State {
    pub error: Option<String>,
    /// None until the first load.
    agents: Option<Vec<AgentRow>>,
    /// The roles the roster holds, as (id, label), sorted by id.
    roles: Vec<(String, String)>,
    role_filter: Option<String>,
    status_filter: StatusFilter,
    pub wizard: wizard::Wizard,
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
    Acted(Result<(), String>),
    Wizard(WizardMsg),
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

const STATUS_CHOICES: [StatusFilter; 4] = [StatusFilter::All, StatusFilter::Working, StatusFilter::Idle, StatusFilter::Offline];

impl StatusFilter {
    fn matches(self, status: &str) -> bool {
        match self {
            StatusFilter::All => true,
            StatusFilter::Working => status == "working",
            StatusFilter::Idle => status == "idle",
            StatusFilter::Offline => status == "offline",
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
    status: String,
    model: String,
    model_source: String,
    tokens: u64,
    /// The work it is doing now: (kind, prompt).
    work: Option<(String, String)>,
}

fn parse(v: &Value) -> Option<AgentRow> {
    let cid = v.get("cid")?.as_u64()?;
    let field = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    let usage = |key: &str| v.get("usage").and_then(|u| u.get(key)).and_then(Value::as_u64).unwrap_or(0);
    let work = match v.get("work") {
        Some(w) if w.is_object() => Some((
            w.get("kind").and_then(Value::as_str).unwrap_or("").to_string(),
            w.get("prompt").and_then(Value::as_str).unwrap_or("").to_string(),
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
        status: field("status"),
        model: field("model"),
        model_source: field("model_source"),
        tokens: usage("input_tokens") + usage("output_tokens"),
        work,
    })
}

/// The roles a roster holds: (id, label), sorted by id, each once.
fn roles_of(list: &[AgentRow]) -> Vec<(String, String)> {
    let mut roles: Vec<(String, String)> = list.iter().map(|a| (a.role.clone(), a.role_label.clone())).collect();
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

/// Loads the screen's data (called when it opens and on every poll tick).
pub fn load(_state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(move || client.get::<Value>("/v1/agents"), Msg::Loaded)
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(Ok(v)) => {
            let list: Vec<AgentRow> = v.as_array().map(|items| items.iter().filter_map(parse).collect()).unwrap_or_default();
            state.roles = roles_of(&list);
            if state.role_filter.as_ref().is_some_and(|r| !state.roles.iter().any(|(id, _)| id == r)) {
                state.role_filter = None;
            }
            state.agents = Some(list);
            state.error = None;
            Task::none()
        }
        Msg::Loaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
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
            client::task(move || client.call("POST", &format!("/v1/agents/{cid}/cancel"), None).map(|_| ()), Msg::Acted)
        }
        Msg::Stop(cid) => {
            let client = ctx.client.clone();
            client::task(move || client.call("POST", &format!("/v1/agents/{cid}/stop"), None).map(|_| ()), Msg::Acted)
        }
        Msg::Acted(Ok(())) => load(state, ctx),
        Msg::Acted(Err(e)) => {
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
    let page = ui::sheet(head(state), list(state));
    if state.wizard.open {
        Stack::new().push(page).push(wizard::view(&state.wizard, ctx).map(Msg::Wizard)).into()
    } else {
        page
    }
}

fn spawn_button<'a>() -> Element<'a, Msg> {
    ui::btn("Spawn agents", Some("plus"), Kind::Primary, Some(Msg::Wizard(WizardMsg::Open)))
}

fn head<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut role_options = vec![RoleChoice { id: None, label: "All roles".to_string() }];
    role_options.extend(state.roles.iter().map(|(id, label)| RoleChoice { id: Some(id.clone()), label: label.clone() }));
    let selected_role = role_options.iter().find(|c| c.id == state.role_filter).cloned();
    ui::view_head(
        "Agents",
        vec![
            pick_list(role_options, selected_role, |c: RoleChoice| Msg::SetRole(c.id)).into(),
            pick_list(STATUS_CHOICES.to_vec(), Some(state.status_filter), Msg::SetStatus).into(),
            spawn_button(),
        ],
    )
}

fn list<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(e) = &state.error {
        parts.push(ui::error_line(e));
    }
    match &state.agents {
        None => parts.push(ui::loading()),
        Some(all) if all.is_empty() => parts.push(
            column![ui::empty("agents", "No agents connected", "Spawn agents to fill the roster."), spawn_button()]
                .align_x(Alignment::Center)
                .into(),
        ),
        Some(all) => {
            let rows: Vec<&AgentRow> = all
                .iter()
                .filter(|a| state.role_filter.as_ref().map_or(true, |r| *r == a.role) && state.status_filter.matches(&a.status))
                .collect();
            if rows.is_empty() {
                parts.push(
                    column![
                        ui::empty("search", "No agents match", "Clear the filters to see the roster."),
                        ui::btn("Clear filters", None, Kind::Ghost, Some(Msg::ClearFilters)),
                    ]
                    .align_x(Alignment::Center)
                    .into(),
                );
            } else {
                parts.push(table(rows));
            }
        }
    }
    column(parts).spacing(12).into()
}

fn table<'a>(rows: Vec<&'a AgentRow>) -> Element<'a, Msg> {
    let header = row![
        cell(3, ui::label("Agent").into()),
        cell(2, ui::label("Model").into()),
        cell(3, ui::label("Work").into()),
        Element::from(container(ui::label("Tokens")).width(Length::FillPortion(1)).align_x(Alignment::End)),
        Element::from(Space::new().width(Length::Fixed(ACTIONS_WIDTH))),
    ]
    .spacing(12)
    .padding([8, 12])
    .align_y(Alignment::Center);
    let mut lines: Vec<Element<'a, Msg>> = vec![header.into(), ui::hairline()];
    for a in rows {
        lines.push(row_view(a));
        lines.push(ui::hairline());
    }
    scrollable(column(lines)).width(Length::Fill).into()
}

const ACTIONS_WIDTH: f32 = 124.0;

fn cell<'a>(portion: u16, content: Element<'a, Msg>) -> Element<'a, Msg> {
    container(content).width(Length::FillPortion(portion)).into()
}

fn row_view<'a>(a: &'a AgentRow) -> Element<'a, Msg> {
    let line = row![
        cell(3, agent_cell(a)),
        cell(2, model_cell(a)),
        cell(3, work_cell(a)),
        Element::from(container(ui::mono(compact(a.tokens), 12.0, INK)).width(Length::FillPortion(1)).align_x(Alignment::End)),
        Element::from(container(actions(a)).width(Length::Fixed(ACTIONS_WIDTH)).align_x(Alignment::End)),
    ]
    .spacing(12)
    .padding([10, 12])
    .align_y(Alignment::Center);
    mouse_area(line).on_press(Msg::Go(Id::Agent(a.cid))).into()
}

fn agent_cell<'a>(a: &'a AgentRow) -> Element<'a, Msg> {
    let mark: Element<'a, Msg> = if a.orchestrator { ui::hive(24.0) } else { ui::icon("agents", 20.0, INK_3) };
    let portrait = container(mark)
        .width(Length::Fixed(40.0))
        .height(Length::Fixed(40.0))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(|_: &Theme| container::Style {
            border: Border { color: RULE, width: 1.0, radius: 0.0.into() },
            ..Default::default()
        });
    let name = if a.name.trim().is_empty() { "Unnamed".to_string() } else { a.name.clone() };
    let name_line = row![dot(status_tone(&a.status)), ui::mono(name, 13.0, INK)].spacing(8).align_y(Alignment::Center);
    row![portrait, column![name_line, ui::faint(a.role_label.clone())].spacing(4)]
        .spacing(12)
        .align_y(Alignment::Center)
        .into()
}

fn model_cell<'a>(a: &'a AgentRow) -> Element<'a, Msg> {
    let provider = a.model.split(['/', ':']).next().unwrap_or("");
    row![
        ui::brand_logo(provider, 14.0),
        ui::mono(a.model.clone(), 11.5, INK_2),
        tag(a.orchestrator, "orchestrator", Tone::Signal),
        tag(a.model_source != "default", if a.model_source == "agent" { "own" } else { "role" }, Tone::Muted),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

fn work_cell<'a>(a: &'a AgentRow) -> Element<'a, Msg> {
    match &a.work {
        Some((kind, prompt)) => row![ui::badge(kind, work_tone(kind)), ui::faint(ui_clip(prompt, 56))]
            .spacing(8)
            .align_y(Alignment::Center)
            .into(),
        None => ui::faint("—").into(),
    }
}

fn actions<'a>(a: &'a AgentRow) -> Element<'a, Msg> {
    let cid = a.cid;
    let mut buttons = row![ui::btn("", Some("conversations"), Kind::Ghost, Some(Msg::Go(Id::Agent(cid))))].spacing(4);
    if a.status == "working" {
        buttons = buttons.push(ui::btn("", Some("ban"), Kind::Ghost, Some(Msg::Cancel(cid))));
    }
    buttons.push(ui::btn("", Some("stop"), Kind::Ghost, Some(Msg::Stop(cid)))).into()
}

/// A small square status dot in a tone.
fn dot<'a>(tone: Tone) -> Element<'a, Msg> {
    let color = tone.color();
    container(Space::new().width(Length::Fixed(8.0)).height(Length::Fixed(8.0)))
        .style(move |_: &Theme| container::Style {
            background: Some(color.into()),
            ..Default::default()
        })
        .into()
}

/// A badge when `on`, nothing otherwise.
fn tag<'a>(on: bool, label: &str, tone: Tone) -> Element<'a, Msg> {
    if on {
        ui::badge(label, tone)
    } else {
        Space::new().width(Length::Fixed(0.0)).into()
    }
}

fn status_tone(status: &str) -> Tone {
    match status {
        "working" => Tone::Info,
        "idle" => Tone::Ok,
        _ => Tone::Muted,
    }
}

fn work_tone(kind: &str) -> Tone {
    match kind {
        "task" => Tone::Signal,
        "chat" => Tone::Info,
        "motion" => Tone::Warn,
        _ => Tone::Muted,
    }
}

/// A count in thousands or millions, as the web's `compact`.
fn compact(n: u64) -> String {
    if n < 1_000 {
        n.to_string()
    } else if n < 1_000_000 {
        let k = n as f64 / 1_000.0;
        if n < 10_000 { format!("{k:.1}k") } else { format!("{k:.0}k") }
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
