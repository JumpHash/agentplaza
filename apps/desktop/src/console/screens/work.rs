//! The Work screen (parity with the web console's `work` view, docs/design/parity.md row 19): a project as one flow.
//! Its planning (brief, research, proposal, critique, approval), then its tasks in the order their `after` links let
//! them run, then delivery. The tasks are laid out natively by `crates/apz-graph`, the layout the web runs as wasm (a
//! test lays the fixtures out both ways); each card says who has the task and what they are doing now, or what it
//! waits for. "Only what needs me" keeps the owner's decisions and the work they hold up. Cards never move while a
//! project's graph is unchanged, and done work dims in place. Tasks outside projects have a tab of their own.
//!
//! The map is a canvas of frames and links under the cards' own widgets, pinned where the layout puts them: the canvas
//! draws what a widget's border cannot (dashed frames, the links), the widgets carry the text and take the clicks. The
//! rules are in `work_model.rs`.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::canvas::{self, Canvas, Frame, Path, Stroke};
use iced::widget::scrollable::{AbsoluteOffset, Viewport};
use iced::widget::text::Wrapping;
use iced::widget::{Space, button, checkbox, column, container, pin, row, stack, text};
use iced::{
    Alignment, Border, Color, Element, Length, Padding, Point, Rectangle, Renderer, Shadow, Size,
    Subscription, Task, Theme, Vector, mouse,
};
use serde::Deserialize;
use serde_json::json;

use super::Id;
use super::work_model::{
    self as model, FLOW_TASKS, GraphTask, LineContext, PlanCard, PlanState, Project, Seg, SegState,
    Step, WorkState,
};
use crate::console::{Ctx, boot, client, events, roles, ui};
use crate::theme;

/// The desk sizes of a task card and its gaps: the web's `CARD` (apps/web/src/views/work.ts). A test holds the two
/// equal, so both consoles lay a graph out alike. A card is tall enough for a two-line title between its id row, its
/// line and its state.
pub const CARD: apz_graph::Options = apz_graph::Options {
    card_width: 232.0,
    card_height: 132.0,
    gap_x: 48.0,
    gap_y: 14.0,
    pad: 12.0,
};
/// The Plan column's cards and delivery, as the web's `PW`, `PH`, `PG` and `GX` (delivery is a plan card's size).
const PLAN_W: f64 = 220.0;
const PLAN_H: f64 = 72.0;
const PLAN_GAP: f64 = 14.0;
const SIDE_GAP: f64 = 48.0;
const DELIVERY: (f64, f64) = (220.0, 72.0);
/// The map's margin inside its frame (the web's `.wk-canvas` margin, `--s5`).
const MARGIN: f32 = 20.0;
const DRAWER_W: f32 = 340.0;
/// The space between the map and the drawer.
const DRAWER_GAP: f32 = 16.0;
/// The tab of the tasks outside projects (the web's `#/work/outside`).
pub const LOOSE: &str = "outside";
/// The saved setting that remembers "Only what needs me" (the web keeps it in localStorage).
const LENS_KEY: &str = "work_lens";
/// About how many characters fit a card's two-line title and its one-line line, at the card's width: iced cannot
/// clamp text with an ellipsis, so the text is cut first and the box clips what is left.
const TITLE_CHARS: usize = 50;
const LINE_CHARS: usize = 33;
/// The map's scroll area, so a selected card can be scrolled into view.
static MAP_ID: LazyLock<iced::widget::Id> = LazyLock::new(|| iced::widget::Id::new("work-map"));

/// An agent, with what the cards read of it: its name, and its step now.
#[derive(Debug, Clone, Deserialize)]
pub struct Agent {
    pub cid: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub step: Option<Step>,
}

#[derive(Deserialize)]
struct TasksReply {
    tasks: Vec<model::Task>,
}

/// One read of the hub: the projects, the ledger, the agents and the open project's graph, and the tab they were read
/// for.
#[derive(Debug, Clone)]
pub struct Snapshot {
    projects: Vec<Project>,
    tasks: Vec<model::Task>,
    agents: Vec<Agent>,
    graph: Option<model::Graph>,
    current: String,
}

#[derive(Debug, Default)]
pub struct State {
    pub error: Option<String>,
    loaded: bool,
    /// The open tab: a project's id, or `LOOSE`. Empty until the first read picks one.
    current: String,
    /// The tab the screen was opened on (`Id::WorkOn`), until the first read.
    wanted: Option<String>,
    projects: Vec<Project>,
    tasks: Vec<model::Task>,
    agents: Vec<Agent>,
    /// The open project's graph, as the hub's read model gives it; None on the tab of the tasks outside projects.
    graph: Option<model::Graph>,
    /// The shown graph's layout, and the key of the tasks and links it was made for: kept while they are the same, so
    /// cards never move under the reader.
    layout: Option<(String, apz_graph::Layout)>,
    /// The task in the drawer.
    selected: Option<String>,
    /// The selected task as the ledger reads it: when it was posted, its note and outcome.
    detail: Option<model::Task>,
    /// "Only what needs me"; None until it is read from the saved settings.
    lens: Option<bool>,
    /// The agents' steps as `agent_step` events bring them, newer than the roster's until the next read.
    steps: HashMap<u64, Option<Step>>,
    /// The map's scroll offset and width, as it last reported them.
    viewport: Option<(f32, f32)>,
    /// A read is in flight, and whether a hub event asked for another meanwhile.
    loading: bool,
    again: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Result<Snapshot, String>),
    /// A tab of the strip: a project's id, or `LOOSE`.
    Tab(String),
    /// A card, or a link in the drawer: the task the drawer shows; None closes it.
    Select(Option<String>),
    /// The selected task, as the ledger reads it.
    Detail(String, Result<model::Task, String>),
    /// "Only what needs me".
    Lens(bool),
    /// The map scrolled, or its size changed.
    MapScrolled(Viewport),
    /// A hub event: a project, a task or a link changed, or an agent's step moved.
    Event(events::Event),
    /// Escape closes the drawer.
    Escape,
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// The screen as it opens on one tab (a project's id, or `LOOSE`): the Projects screen's "Open in Work" and the
/// Overview's project lines open it this way.
pub fn opened(tab: String) -> State {
    State {
        wanted: Some(tab),
        ..Default::default()
    }
}

/// Opens the drawer on a task before the first read (`APZ_WORK_TASK`, a capture hook): the drawer shows once the task
/// is on the screen.
pub fn capture_select(state: &mut State, id: String) {
    state.selected = Some(id);
}

/// Loads the screen's data (called when it opens, on every poll tick and on a hub event).
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    if state.lens.is_none() {
        state.lens = Some(
            boot::saved_setting(LENS_KEY)
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        );
    }
    state.loading = true;
    let hub = ctx.client.clone();
    // A tab asked for, else the project the console is scoped to.
    let wanted = state.wanted.clone().or_else(|| ctx.project.clone());
    let current = state.current.clone();
    client::task(move || snapshot(&hub, current, wanted), Msg::Loaded)
}

/// One read: the projects (the active ones first, the newest first), the ledger, the agents, and the graph of the tab
/// it picks: the one open, else the one wanted, else the first project still in progress, else the first, else the
/// tasks outside projects.
fn snapshot(
    hub: &client::Client,
    current: String,
    wanted: Option<String>,
) -> Result<Snapshot, String> {
    let mut projects: Vec<Project> = hub.get("/v1/projects")?;
    projects.sort_by(|a, b| {
        (a.stage == "delivered")
            .cmp(&(b.stage == "delivered"))
            .then(b.updated_ms.cmp(&a.updated_ms))
    });
    let tasks = hub.get::<TasksReply>("/v1/tasks")?.tasks;
    // Names and steps: without them the cards still draw, with "agent 7" and no step.
    let agents: Vec<Agent> = hub.get("/v1/agents").unwrap_or_default();
    let current = if current.is_empty() {
        wanted
            .filter(|w| !w.is_empty())
            .or_else(|| {
                projects
                    .iter()
                    .find(|p| p.stage != "delivered")
                    .or(projects.first())
                    .map(|p| p.id.clone())
            })
            .unwrap_or_else(|| {
                if tasks.iter().any(|t| t.project.is_none()) {
                    LOOSE.to_string()
                } else {
                    String::new()
                }
            })
    } else {
        current
    };
    let graph = if current != LOOSE && projects.iter().any(|p| p.id == current) {
        Some(hub.get(&format!("/v1/projects/{}/graph", client::enc(&current)))?)
    } else {
        None
    };
    Ok(Snapshot {
        projects,
        tasks,
        agents,
        graph,
        current,
    })
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(Ok(snap)) => {
            state.error = None;
            state.loaded = true;
            state.wanted = None;
            state.projects = snap.projects;
            state.tasks = snap.tasks;
            state.agents = snap.agents;
            // The roster was read after the steps the events brought, so its own are as new.
            state.steps.clear();
            // A tab picked while this read was in flight wins: its own read brings its graph.
            if state.current.is_empty() || state.current == snap.current {
                state.current = snap.current;
                state.graph = snap.graph;
            }
            keep_layout(state);
            landed(state, ctx)
        }
        Msg::Loaded(Err(e)) => {
            state.error = Some(e);
            landed(state, ctx)
        }
        Msg::Tab(key) => {
            if key == state.current {
                return Task::none();
            }
            state.current = key;
            state.graph = None;
            state.layout = None;
            state.selected = None;
            state.detail = None;
            state.viewport = None;
            load(state, ctx)
        }
        Msg::Select(None) | Msg::Escape => {
            state.selected = None;
            state.detail = None;
            Task::none()
        }
        Msg::Select(Some(id)) => {
            let opening = state.selected.is_none();
            state.selected = Some(id.clone());
            state.detail = None;
            let reveal = reveal(state, &id, opening);
            let hub = ctx.client.clone();
            let path = format!("/v1/tasks/{}", client::enc(&id));
            let read = client::task(
                move || hub.get::<model::Task>(&path),
                move |res| Msg::Detail(id.clone(), res),
            );
            Task::batch([reveal, read])
        }
        Msg::Detail(id, res) => {
            if state.selected.as_deref() == Some(id.as_str()) {
                state.detail = res.ok();
            }
            Task::none()
        }
        Msg::Lens(on) => {
            state.lens = Some(on);
            // A refused write keeps the choice for this run only.
            let _ = boot::save_setting(LENS_KEY, json!(on));
            Task::none()
        }
        Msg::MapScrolled(viewport) => {
            state.viewport = Some((viewport.absolute_offset().x, viewport.bounds().width));
            Task::none()
        }
        Msg::Event(event) => match event.kind.as_str() {
            "project" | "task" | "task_link" => {
                if state.loading {
                    state.again = true;
                    return Task::none();
                }
                load(state, ctx)
            }
            "agent_step" => {
                if let Some(cid) = event.data.get("cid").and_then(|c| c.as_u64()) {
                    let step = event
                        .data
                        .get("step")
                        .cloned()
                        .and_then(|s| serde_json::from_value::<Step>(s).ok());
                    state.steps.insert(cid, step);
                }
                Task::none()
            }
            _ => Task::none(),
        },
    }
}

/// A read landed: the read that a hub event asked for meanwhile runs now.
fn landed(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    state.loading = false;
    if std::mem::take(&mut state.again) {
        load(state, ctx)
    } else {
        Task::none()
    }
}

/// The hub's events while the screen is open, and Escape, which closes the drawer.
pub fn subscription(ctx: &Ctx) -> Subscription<Msg> {
    Subscription::batch([
        events::subscription(&ctx.client).map(Msg::Event),
        iced::event::listen_with(keys),
    ])
}

fn keys(event: iced::Event, status: iced::event::Status, _window: iced::window::Id) -> Option<Msg> {
    use iced::keyboard::key::Named;
    match event {
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Named(Named::Escape),
            ..
        }) if status == iced::event::Status::Ignored => Some(Msg::Escape),
        _ => None,
    }
}

// ---- What the screen shows -------------------------------------------------------------------------------------------

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// The tasks the open tab shows, with their links: the project's graph, or the tasks outside projects.
fn shown(state: &State) -> model::Graph {
    if state.current == LOOSE {
        model::loose_graph(&state.tasks)
    } else {
        state.graph.clone().unwrap_or_default()
    }
}

/// The layout's key: the shown tasks and links. The same key keeps the same layout.
fn layout_key(g: &model::Graph) -> String {
    let ids: Vec<&str> = g.tasks.iter().map(|t| t.id.as_str()).collect();
    let links: Vec<String> = g.edges.iter().map(|[a, b]| format!("{a}>{b}")).collect();
    format!("{}|{}", ids.join(","), links.join(","))
}

/// Lays the shown graph out when its tasks or links changed; a graph with no links stands in state columns instead.
fn keep_layout(state: &mut State) {
    let g = shown(state);
    if g.edges.is_empty() {
        state.layout = None;
        return;
    }
    let key = layout_key(&g);
    if state.layout.as_ref().is_some_and(|(k, _)| *k == key) {
        return;
    }
    let input = apz_graph::Graph {
        tasks: g
            .tasks
            .iter()
            .map(|t| apz_graph::Task { id: t.id.clone() })
            .collect(),
        edges: g.edges.clone(),
    };
    state.layout = Some((key, apz_graph::layout(&input, &CARD)));
}

/// An agent's step now: the latest event's, else the roster's.
fn step_of(state: &State, cid: u64) -> Option<Step> {
    match state.steps.get(&cid) {
        Some(step) => step.clone(),
        None => state
            .agents
            .iter()
            .find(|a| a.cid == cid)
            .and_then(|a| a.step.clone()),
    }
}

/// An agent's name, or "agent 7" until it chooses one (as the web's Work screen says it).
fn name_of(state: &State, cid: u64) -> String {
    match state.agents.iter().find(|a| a.cid == cid) {
        Some(a) if !a.name.trim().is_empty() => a.name.clone(),
        _ => format!("agent {cid}"),
    }
}

fn state_of(state: &State, t: &GraphTask) -> WorkState {
    let step = t.agent_cid.and_then(|cid| step_of(state, cid));
    WorkState::of(&t.state, t.ready, &t.blocked_by, step.as_ref())
}

/// The ledger by task id, built once for a drawing: each card asks it when its task became ready.
fn ledger(state: &State) -> HashMap<&str, &model::Task> {
    state.tasks.iter().map(|t| (t.id.as_str(), t)).collect()
}

/// When a task in the ledger became ready; a task the ledger list does not have yet has no time.
fn ready_at(ledger: &HashMap<&str, &model::Task>, id: &str) -> Option<i64> {
    ledger.get(id).map(|t| model::ready_since(t, ledger))
}

fn lens_on(state: &State) -> bool {
    state.lens.unwrap_or(false)
}

/// The geometry of the map: the plan column (for a project), the graph, then delivery.
fn geometry(p: Option<&Project>, laid: &apz_graph::Layout) -> model::MapGeometry {
    let plan = p.map(|_| {
        let n = model::PLAN_STAGES.len() as f64;
        // The web's `planH`: a card and its gap for each stage.
        model::PlanPart {
            width: PLAN_W,
            height: n * (PLAN_H + PLAN_GAP),
        }
    });
    model::compose(laid, plan, p.map(|_| DELIVERY), SIDE_GAP)
}

/// Scrolls the map so the selected card is in view, as the web's `scrollIntoView({inline: 'nearest'})`: the drawer
/// narrows the map when it opens, and a link in the drawer can name a task scrolled out of sight. Nothing when the map
/// has not reported its viewport (it fits, so every card is in view).
fn reveal(state: &State, id: &str, opening: bool) -> Task<Msg> {
    let (Some((_, laid)), Some((offset, width))) = (state.layout.as_ref(), state.viewport) else {
        return Task::none();
    };
    let Some(card) = laid.cards.iter().find(|c| c.id == id) else {
        return Task::none();
    };
    let p = state.projects.iter().find(|p| p.id == state.current);
    let geo = geometry(p, laid);
    let left = MARGIN + (geo.graph[0] + card.x) as f32;
    let right = left + card.width as f32;
    let visible = if opening {
        width - DRAWER_W - DRAWER_GAP
    } else {
        width
    };
    let target = if left < offset {
        left - MARGIN
    } else if right > offset + visible {
        right - visible + MARGIN
    } else {
        return Task::none();
    };
    iced::widget::operation::scroll_to(
        MAP_ID.clone(),
        AbsoluteOffset {
            x: Some(target.max(0.0)),
            y: None,
        },
    )
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let toggle = checkbox(lens_on(state))
        .label("Only what needs me")
        .on_toggle(Msg::Lens)
        .size(14.0)
        .text_size(13.0);
    let head = ui::view_head_with_intro(
        "Work",
        "A project as one flow: its plan, its tasks in the order they can run, and delivery.",
        vec![toggle.into()],
    );
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(e) = &state.error {
        parts.push(ui::error_line(e));
    }
    if !state.loaded {
        parts.push(ui::loading());
    } else if state.projects.is_empty() && !state.tasks.iter().any(|t| t.project.is_none()) {
        parts.push(ui::empty(
            "projects",
            "No projects yet",
            "The orchestrator opens a project for each goal you give it; its work shows here.",
        ));
    } else {
        parts.push(strip(state));
        let g = shown(state);
        let main = body(state, &g);
        parts.push(match drawer(state, &g) {
            Some(side) => row![container(main).width(Length::Fill), side]
                .spacing(DRAWER_GAP)
                .into(),
            None => main,
        });
    }
    ui::sheet(head, column(parts).spacing(18).into())
}

// ---- The strip: a tab per project, with its flow line ----------------------------------------------------------------

fn strip<'a>(state: &'a State) -> Element<'a, Msg> {
    let steps = |cid: u64| step_of(state, cid);
    let count = |n: usize| format!("{n} {}", if n == 1 { "task" } else { "tasks" });
    let mut tabs: Vec<Element<'a, Msg>> = state
        .projects
        .iter()
        .map(|p| {
            let own = model::tasks_of(p, &state.tasks);
            let sub = if model::waits_for_owner(Some(p), &own, &steps) {
                "waits for you".to_string()
            } else {
                count(own.len())
            };
            tab(
                &p.id,
                &p.title,
                format!("{} · {sub}", model::stage_label(&p.stage)),
                &model::flow_segments(Some(p), &own, &steps),
                own.len().saturating_sub(FLOW_TASKS),
                p.id == state.current,
            )
        })
        .collect();
    let loose: Vec<&model::Task> = state.tasks.iter().filter(|t| t.project.is_none()).collect();
    if !loose.is_empty() {
        let sub = if model::waits_for_owner(None, &loose, &steps) {
            "waits for you".to_string()
        } else {
            count(loose.len())
        };
        tabs.push(tab(
            LOOSE,
            "Outside projects",
            sub,
            &model::flow_segments(None, &loose, &steps),
            loose.len().saturating_sub(FLOW_TASKS),
            state.current == LOOSE,
        ));
    }
    // The strip's rule runs under the tabs; the open tab's signal underline sits on it (the web's `.wk-strip`).
    let rule = container(Space::new().height(1.0))
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(theme::palette().rule.into()),
            ..Default::default()
        });
    column![
        ui::scroll_x(container(row(tabs).spacing(32)).padding(Padding {
            top: 0.0,
            right: 4.0,
            bottom: 0.0,
            left: 0.0,
        })),
        rule
    ]
    .into()
}

/// A tab of the strip: its title, its line and its flow, with a signal underline when it is open (the web's `.wk-tab`).
fn tab<'a>(
    key: &str,
    title: &str,
    sub: String,
    segs: &[Seg],
    more: usize,
    current: bool,
) -> Element<'a, Msg> {
    let pal = theme::palette();
    let body = column![
        text(ui::ellipsize(title, 30))
            .font(ui::SANS_SEMIBOLD)
            .size(13.0)
            .color(pal.ink)
            .wrapping(Wrapping::None),
        ui::sans(sub, 12.0, pal.ink_3).wrapping(Wrapping::None),
        flow_line(segs, more),
    ]
    .spacing(4);
    let underline = container(Space::new().height(2.0))
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: current.then(|| theme::palette().signal.into()),
            ..Default::default()
        });
    button(column![body, underline].spacing(8))
        .padding(Padding {
            top: 8.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        })
        .width(Length::Fixed(200.0))
        .style(move |_: &Theme, _status| {
            let pal = theme::palette();
            button::Style {
                background: None,
                text_color: if current { pal.ink } else { pal.ink_2 },
                ..Default::default()
            }
        })
        .on_press(Msg::Tab(key.to_string()))
        .into()
}

/// The console's hard offset shadow (the web's `--print`): black on the dark ground, ink on the light one. A hair of
/// blur keeps iced drawing the box's text (it drops it under a shadow of exactly zero blur).
fn print() -> Shadow {
    Shadow {
        color: if theme::is_light() {
            theme::palette().ink
        } else {
            Color::BLACK
        },
        offset: Vector::new(2.0, 2.0),
        blur_radius: 0.6,
    }
}

/// A flow line: the plan's squares, a square per task, then review and delivery (and "+N" past `FLOW_TASKS`). The
/// Overview's project lines draw it too.
pub fn flow_line<'a, M: 'a>(segs: &[Seg], more: usize) -> Element<'a, M> {
    let mut items: Vec<Element<'a, M>> = Vec::new();
    for s in segs {
        // Every square is 9px and they sit 3px apart, as the web's `.wk-sq` in `.wk-flow`.
        let (fill, edge) = square_colors(s.state);
        items.push(
            container(Space::new().width(9.0).height(9.0))
                .style(move |_: &Theme| container::Style {
                    background: fill.map(Into::into),
                    border: Border {
                        color: edge,
                        width: 1.0,
                        radius: 2.0.into(),
                    },
                    snap: true,
                    ..Default::default()
                })
                .into(),
        );
    }
    if more > 0 {
        items.push(ui::sans(format!("+{more}"), 11.0, theme::palette().ink_3).into());
    }
    row(items).spacing(3).align_y(Alignment::Center).into()
}

/// A flow square's fill and edge. Orange is the owner's: only what waits on the owner wears it. The web dashes the
/// squares of what is still to come; here they are drawn in the faint rule.
fn square_colors(state: SegState) -> (Option<Color>, Color) {
    let pal = theme::palette();
    match state {
        SegState::Done => (Some(pal.ink_3), pal.ink_3),
        SegState::Current => (Some(pal.ink), pal.ink),
        SegState::Later | SegState::Task(WorkState::Waiting) => (None, pal.rule),
        SegState::You | SegState::Task(WorkState::NeedsYou) => (Some(pal.signal), pal.signal),
        SegState::Task(WorkState::Done) => (Some(pal.ok), pal.ok),
        SegState::Task(WorkState::Ready) => (None, pal.info),
        SegState::Task(WorkState::Working | WorkState::Review) => (Some(pal.info), pal.info),
        SegState::Task(WorkState::Blocked | WorkState::Failed) => (Some(pal.crit), pal.crit),
        SegState::Task(WorkState::Withdrawn) => (None, pal.ink_3),
    }
}

// ---- The body: the map, the state columns, or the planning stages ---------------------------------------------------

fn body<'a>(state: &'a State, g: &model::Graph) -> Element<'a, Msg> {
    if state.current == LOOSE {
        if g.tasks.is_empty() {
            return ui::empty("tasks", "No tasks outside projects", "");
        }
        return map_or_columns(state, g, None);
    }
    let Some(p) = state.projects.iter().find(|p| p.id == state.current) else {
        return ui::empty("projects", "No such project", "It may have been removed.");
    };
    let building = model::stage_index(&p.stage) >= model::stage_index("implementation");
    if building && state.graph.is_none() {
        return ui::loading();
    }
    if !building || g.tasks.is_empty() {
        return planning(p);
    }
    map_or_columns(state, g, Some(p))
}

/// The tasks' states on the screen, by id.
fn states(state: &State, g: &model::Graph) -> HashMap<String, WorkState> {
    g.tasks
        .iter()
        .map(|t| (t.id.clone(), state_of(state, t)))
        .collect()
}

/// The map, or state columns when no task has a link. With the lens on and nothing waiting on the owner, a line says so
/// above them.
fn map_or_columns<'a>(
    state: &'a State,
    g: &model::Graph,
    p: Option<&'a Project>,
) -> Element<'a, Msg> {
    let states = states(state, g);
    let kept = lens_on(state).then(|| model::needs_me(&states, &g.edges));
    let laid = state
        .layout
        .as_ref()
        .filter(|(key, _)| *key == layout_key(g))
        .map(|(_, laid)| laid);
    let shown = match laid {
        Some(laid) if !g.edges.is_empty() => linked_map(state, g, p, laid, &states, kept.as_ref()),
        _ => state_columns(
            state,
            g,
            &states,
            kept.as_ref(),
            "These tasks have no after links yet, so they stand in state columns. A plan’s tasks list gives them their order.",
        ),
    };
    if kept.as_ref().is_none_or(|k| !k.is_empty()) {
        return shown;
    }
    let whose = p.map_or("outside projects".to_string(), |p| {
        format!("in {}", p.title)
    });
    column![
        ui::sans(
            format!("Nothing {whose} needs you now."),
            12.0,
            theme::palette().ink_3
        ),
        shown
    ]
    .spacing(8)
    .into()
}

/// What the canvas under the map draws: the cards' and the plan's frames, and the links.
struct MapPaint {
    lines: Vec<PaintLine>,
    frames: Vec<PaintFrame>,
}

/// A link: a curve from `from` to `to` as the web's `bez` draws it, its colour, whether it is dashed (the plan's and
/// delivery's), and whether it ends in an arrowhead.
struct PaintLine {
    from: Point,
    to: Point,
    color: Color,
    dashed: bool,
    head: bool,
}

/// A box's frame: its ground and edge, and the ring round a picked card (the web's `box-shadow: 0 0 0 1px`).
struct PaintFrame {
    rect: Rectangle,
    fill: Color,
    edge: Color,
    ring: Option<Color>,
}

const DASH: &[f32] = &[4.0, 4.0];

impl canvas::Program<Msg> for MapPaint {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry<Renderer>> {
        let mut frame = Frame::new(renderer, bounds.size());
        for line in &self.lines {
            // The web's `bez`: out to the middle, then in, with both ends level.
            let mid = (line.from.x + line.to.x) / 2.0;
            let path = Path::new(|p| {
                p.move_to(line.from);
                p.bezier_curve_to(
                    Point::new(mid, line.from.y),
                    Point::new(mid, line.to.y),
                    line.to,
                );
            });
            let stroke = Stroke::default().with_color(line.color).with_width(1.5);
            let stroke = if line.dashed {
                Stroke {
                    line_dash: canvas::LineDash {
                        segments: DASH,
                        offset: 0,
                    },
                    ..stroke
                }
            } else {
                stroke
            };
            frame.stroke(&path, stroke);
            if line.head {
                let end = line.to;
                let head = Path::new(|p| {
                    p.move_to(Point::new(end.x - 6.0, end.y - 4.0));
                    p.line_to(end);
                    p.line_to(Point::new(end.x - 6.0, end.y + 4.0));
                });
                frame.stroke(&head, stroke);
            }
        }
        for f in &self.frames {
            if let Some(ring) = f.ring {
                // A 1px ring outside the box: the fill below covers all but it.
                frame.fill_rectangle(
                    f.rect.position() + Vector::new(-1.0, -1.0),
                    Size::new(f.rect.width + 2.0, f.rect.height + 2.0),
                    ring,
                );
            }
            frame.fill_rectangle(f.rect.position(), f.rect.size(), f.fill);
            // The edge on the half pixel, so a 1px line stays crisp.
            let edge = Path::rectangle(
                f.rect.position() + Vector::new(0.5, 0.5),
                Size::new(f.rect.width - 1.0, f.rect.height - 1.0),
            );
            frame.stroke(&edge, Stroke::default().with_color(f.edge).with_width(1.0));
        }
        vec![frame.into_geometry()]
    }
}

/// `color` at `alpha` of its opacity.
fn fade(color: Color, alpha: f32) -> Color {
    Color {
        a: color.a * alpha,
        ..color
    }
}

/// A state's badge tone. Orange (signal) is the owner's hand: only what waits on the owner wears it.
fn tone(state: WorkState) -> ui::Tone {
    match state {
        WorkState::NeedsYou => ui::Tone::Signal,
        WorkState::Ready | WorkState::Working | WorkState::Review => ui::Tone::Info,
        WorkState::Blocked | WorkState::Failed => ui::Tone::Danger,
        WorkState::Done => ui::Tone::Ok,
        WorkState::Waiting | WorkState::Withdrawn => ui::Tone::Muted,
    }
}

/// How a card's frame is drawn in a state, as the web's `.wk-card`: its edge and the ring round a picked card. What
/// waits on the owner has the signal edge (`.s-accent`).
fn card_frame(state: WorkState, selected: bool) -> (Color, Option<Color>) {
    let pal = theme::palette();
    if selected {
        // The picked card: a signal edge and ring, as the web's `.wk-card[aria-pressed="true"]`.
        return (pal.signal, Some(pal.signal));
    }
    match state {
        WorkState::NeedsYou => (pal.signal, None),
        _ => (pal.rule, None),
    }
}

/// The linked map, as the web's `mapOf`: the plan column, the graph as the layout places it, and delivery. Each link is a
/// curve from the card it leaves to the one it enters; the links from the plan and to delivery are dashed.
fn linked_map<'a>(
    state: &'a State,
    g: &model::Graph,
    p: Option<&'a Project>,
    laid: &apz_graph::Layout,
    states: &HashMap<String, WorkState>,
    kept: Option<&HashSet<String>>,
) -> Element<'a, Msg> {
    let pal = theme::palette();
    let geo = geometry(p, laid);
    let (gx, gy) = (geo.graph[0] as f32, geo.graph[1] as f32);
    let at = |x: f64, y: f64| Point::new(MARGIN + x as f32, MARGIN + y as f32);
    let mut paint = MapPaint {
        lines: Vec::new(),
        frames: Vec::new(),
    };
    let faded = |id: &str| kept.is_some_and(|k| !k.contains(id));
    // A card's box on the map: its left, top, width and height.
    let box_of = |id: &str| {
        laid.cards
            .iter()
            .find(|c| c.id == id)
            .map(|c| (geo.graph[0] + c.x, geo.graph[1] + c.y, c.width, c.height))
    };
    let link =
        |from: (f64, f64), to: (f64, f64), color: Color, dashed: bool, head: bool| PaintLine {
            from: at(from.0, from.1),
            to: at(to.0, to.1),
            color,
            dashed,
            head,
        };
    for [a, b] in &g.edges {
        let (Some((ax, ay, aw, ah)), Some((bx, by, _, bh))) = (box_of(a), box_of(b)) else {
            continue;
        };
        let ended = states.get(b).is_some_and(|s| s.ended());
        let color = if ended {
            fade(pal.rule_strong, 0.35)
        } else {
            pal.rule_strong
        };
        paint.lines.push(link(
            (ax + aw, ay + ah / 2.0),
            (bx, by + bh / 2.0),
            color,
            false,
            true,
        ));
    }
    // From the plan to the tasks that run after nothing (the first column), level with the plan's middle.
    if let (Some(plan), Some(first)) = (geo.plan, laid.columns.first()) {
        let start_y = model::MAP_TOP + plan.height / 2.0 - PLAN_GAP;
        for id in first {
            if let Some((bx, by, _, bh)) = box_of(id.as_str()) {
                paint.lines.push(link(
                    (plan.x + plan.width, start_y),
                    (bx, by + bh / 2.0),
                    pal.ink_3,
                    true,
                    false,
                ));
            }
        }
    }
    // From each task that nothing runs after to delivery, which sits level with the map's middle.
    if let Some(d) = geo.delivery {
        let mid = geo.height / 2.0;
        for c in &laid.cards {
            if g.edges.iter().any(|edge| edge[0] == c.id) {
                continue;
            }
            paint.lines.push(link(
                (
                    geo.graph[0] + c.x + c.width,
                    geo.graph[1] + c.y + c.height / 2.0,
                ),
                (d.x, mid),
                pal.ink_3,
                true,
                false,
            ));
        }
    }

    let mut layers: Vec<Element<'a, Msg>> = Vec::new();
    // The plan column: a card per planning stage, each opening the project's page.
    if let (Some(p), Some(plan)) = (p, geo.plan) {
        for (i, c) in model::plan_cards(p).into_iter().enumerate() {
            let rect = Rectangle::new(
                at(plan.x, plan.y + i as f64 * (PLAN_H + PLAN_GAP)),
                Size::new(PLAN_W as f32, PLAN_H as f32),
            );
            paint.frames.push(PaintFrame {
                rect,
                fill: pal.raised,
                edge: if c.state == PlanState::You {
                    pal.signal
                } else {
                    pal.rule
                },
                ring: None,
            });
            layers.push(pinned(
                rect,
                plan_card(&c),
                Msg::Go(Id::Project(p.id.clone())),
            ));
        }
    }
    // The tasks, where the layout puts them.
    let by_id: HashMap<&str, &GraphTask> = g.tasks.iter().map(|t| (t.id.as_str(), t)).collect();
    let hub_state = |id: &str| by_id.get(id).map(|t| t.state.clone());
    let as_id = |id: &str| id.to_string();
    let tasks = ledger(state);
    let ready = |id: &str| ready_at(&tasks, id);
    let name = |cid: u64| name_of(state, cid);
    let step = |cid: u64| step_of(state, cid);
    let ctx = LineContext {
        edges: &g.edges,
        state_of: &hub_state,
        task_name: &as_id,
        ready_at: &ready,
        name_of: &name,
        step_of: &step,
        now: now_ms(),
    };
    for c in &laid.cards {
        let Some(t) = by_id.get(c.id.as_str()) else {
            continue;
        };
        let s = states.get(&t.id).copied().unwrap_or(WorkState::Waiting);
        let selected = state.selected.as_deref() == Some(t.id.as_str());
        // A card the lens keeps out is faded (the web's `.wk-faded`).
        let alpha = if faded(&t.id) { 0.35 } else { 1.0 };
        let rect = Rectangle::new(
            Point::new(MARGIN + gx + c.x as f32, MARGIN + gy + c.y as f32),
            Size::new(c.width as f32, c.height as f32),
        );
        let (edge, ring) = card_frame(s, selected);
        paint.frames.push(PaintFrame {
            rect,
            fill: fade(pal.raised, alpha),
            edge: fade(edge, alpha),
            ring: ring.map(|c| fade(c, alpha)),
        });
        let line = model::card_line(t, s, &ctx);
        layers.push(pinned(
            rect,
            card_body(t, s, line, alpha, true),
            Msg::Select(Some(t.id.clone())),
        ));
    }
    // Delivery.
    if let (Some(p), Some(d)) = (p, geo.delivery) {
        let rect = Rectangle::new(at(d.x, d.y), Size::new(d.width as f32, d.height as f32));
        paint.frames.push(PaintFrame {
            rect,
            fill: pal.raised,
            edge: pal.rule,
            ring: None,
        });
        let delivered = p.stage == "delivered";
        let body = column![
            ui::sans(
                if delivered {
                    "Delivered"
                } else {
                    "Review, then delivered"
                },
                13.0,
                pal.ink
            ),
            ui::sans(
                if delivered {
                    "The lead attached the deliverable"
                } else {
                    "Once every task is accepted, the lead attaches the deliverable"
                },
                12.0,
                pal.ink_3
            )
            .wrapping(Wrapping::WordOrGlyph),
        ]
        .spacing(4)
        .padding([10, 12]);
        layers.push(
            pin(container(body).width(rect.width).height(rect.height))
                .x(rect.x)
                .y(rect.y)
                .into(),
        );
    }

    let (width, height) = (
        geo.width as f32 + 2.0 * MARGIN,
        geo.height as f32 + 2.0 * MARGIN,
    );
    let mut all: Vec<Element<'a, Msg>> = vec![
        Canvas::new(paint)
            .width(Length::Fixed(width))
            .height(Length::Fixed(height))
            .into(),
    ];
    all.extend(layers);
    let canvas = stack(all)
        .width(Length::Fixed(width))
        .height(Length::Fixed(height));
    let map = ui::scroll_x(container(canvas).padding(Padding {
        top: 0.0,
        right: 0.0,
        bottom: 8.0,
        left: 0.0,
    }))
    .id(MAP_ID.clone())
    .on_scroll(Msg::MapScrolled)
    .width(Length::Fill);
    // The map's frame: the sheet under a rule, with the skin's large radius (the web's `.wk-scroll`).
    container(map)
        .padding(16)
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(theme::palette().sheet.into()),
            border: Border {
                color: theme::palette().rule,
                width: 1.0,
                radius: theme::skin().radius_lg.into(),
            },
            snap: true,
            ..Default::default()
        })
        .into()
}

/// A box's widgets at its place on the map: a button the size of the box (one pixel in, so the canvas's edge shows),
/// clear but for the pointer's tint.
fn pinned<'a>(rect: Rectangle, body: Element<'a, Msg>, msg: Msg) -> Element<'a, Msg> {
    let face = button(body)
        .padding(0)
        .width(Length::Fixed(rect.width - 2.0))
        .height(Length::Fixed(rect.height - 2.0))
        .style(|_: &Theme, status| button::Style {
            background: matches!(status, button::Status::Hovered | button::Status::Pressed)
                .then(|| fade(theme::palette().hover, 0.55).into()),
            text_color: theme::palette().ink,
            ..Default::default()
        })
        .on_press(msg);
    pin(face).x(rect.x + 1.0).y(rect.y + 1.0).into()
}

fn plan_card<'a>(c: &PlanCard) -> Element<'a, Msg> {
    let pal = theme::palette();
    let ink = match c.state {
        PlanState::Done => pal.ink_2,
        PlanState::Later => pal.ink_3,
        PlanState::Current | PlanState::You => pal.ink,
    };
    let mut title = row![].spacing(6).align_y(Alignment::Center);
    if c.state == PlanState::Done {
        title = title.push(ui::sans("✓", 12.0, pal.ok));
    }
    title = title.push(ui::sans(c.title.clone(), 13.0, ink).wrapping(Wrapping::None));
    container(
        column![
            title,
            ui::sans(ui::ellipsize(&c.line, 30), 12.0, pal.ink_3).wrapping(Wrapping::None)
        ]
        .spacing(2),
    )
    .padding([0, 12])
    .width(Length::Fill)
    .height(Length::Fill)
    .align_y(iced::alignment::Vertical::Center)
    .into()
}

/// A card's text: the id and the role, the title held to two lines, the line, and the state. On the map it fills the
/// card's fixed height (`fixed`); in a state column it is as tall as it needs.
fn card_body<'a>(
    t: &GraphTask,
    s: WorkState,
    line: String,
    alpha: f32,
    fixed: bool,
) -> Element<'a, Msg> {
    let pal = theme::palette();
    let role = t
        .role
        .as_deref()
        .map_or("Any role".to_string(), roles::label);
    let top = row![
        ui::mono(t.id.clone(), 11.0, fade(pal.ink_3, alpha)),
        Space::new().width(Length::Fill),
        ui::sans(ui::ellipsize(&role, 22), 11.0, fade(pal.ink_3, alpha)).wrapping(Wrapping::None),
    ]
    .align_y(Alignment::Center);
    // An ended card's title is a little dimmer (the web's `.wk-card.ended .wk-card-title`).
    let title_alpha = if s.ended() { alpha * 0.78 } else { alpha };
    let title = container(
        text(ui::ellipsize(&model::title_of(&t.goal), TITLE_CHARS))
            .font(ui::SANS_SEMIBOLD)
            .size(theme::TYPE_ITEM)
            .line_height(1.4)
            .color(fade(pal.ink, title_alpha))
            .wrapping(Wrapping::WordOrGlyph),
    )
    .width(Length::Fill)
    .height(if fixed {
        Length::Fixed(40.0)
    } else {
        Length::Shrink
    })
    .clip(true);
    let line = container(
        ui::sans(
            ui::ellipsize(&line, LINE_CHARS),
            12.0,
            fade(pal.ink_2, alpha),
        )
        .wrapping(Wrapping::None),
    )
    .width(Length::Fill)
    .clip(true);
    // The web's card is a column of 4px gaps with 12px padding: the state chip sits under the line.
    column![top, title, line, ui::badge_dot(s.label(), tone(s))]
        .spacing(4)
        .padding(12)
        .width(Length::Fill)
        .into()
}

/// Tasks without links stand in state columns, with a line saying why.
fn state_columns<'a>(
    state: &'a State,
    g: &model::Graph,
    states: &HashMap<String, WorkState>,
    kept: Option<&HashSet<String>>,
    why: &str,
) -> Element<'a, Msg> {
    let pal = theme::palette();
    let by_id: HashMap<&str, &GraphTask> = g.tasks.iter().map(|t| (t.id.as_str(), t)).collect();
    let hub_state = |id: &str| by_id.get(id).map(|t| t.state.clone());
    let as_id = |id: &str| id.to_string();
    let tasks = ledger(state);
    let ready = |id: &str| ready_at(&tasks, id);
    let name = |cid: u64| name_of(state, cid);
    let step = |cid: u64| step_of(state, cid);
    let ctx = LineContext {
        edges: &g.edges,
        state_of: &hub_state,
        task_name: &as_id,
        ready_at: &ready,
        name_of: &name,
        step_of: &step,
        now: now_ms(),
    };
    let columns: Vec<Element<'a, Msg>> = model::STATE_COLUMNS
        .iter()
        .map(|(label, members)| {
            let items: Vec<Element<'a, Msg>> = g
                .tasks
                .iter()
                .filter(|t| states.get(&t.id).is_some_and(|s| members.contains(s)))
                .map(|t| {
                    let s = states[&t.id];
                    let selected = state.selected.as_deref() == Some(t.id.as_str());
                    let alpha = if kept.is_some_and(|k| !k.contains(&t.id)) {
                        0.18
                    } else if s.ended() {
                        0.62
                    } else {
                        1.0
                    };
                    flat_card(t, s, model::card_line(t, s, &ctx), alpha, selected)
                })
                .collect();
            let count = items.len();
            // The column's head: its name at the left, its count at the right, over a rule (the web's `.wk-ch`).
            let head = column![
                row![
                    ui::sans(label.to_string(), 13.0, pal.ink_2),
                    Space::new().width(Length::Fill),
                    ui::sans(count.to_string(), 13.0, pal.ink_3),
                ]
                .align_y(Alignment::Center),
                container(Space::new().height(1.0))
                    .width(Length::Fill)
                    .style(|_: &Theme| container::Style {
                        background: Some(theme::palette().rule.into()),
                        ..Default::default()
                    }),
            ]
            .spacing(4);
            let body: Element<'a, Msg> = if items.is_empty() {
                ui::sans("None.", 12.0, pal.ink_3).into()
            } else {
                column(items).spacing(8).into()
            };
            column![head, body]
                .spacing(10)
                .width(Length::FillPortion(1))
                .into()
        })
        .collect();
    column![
        ui::sans(why.to_string(), 12.0, pal.ink_3),
        row(columns).spacing(14)
    ]
    .spacing(12)
    .into()
}

/// A card in a state column, as the web's `.wk-card` there: a bordered button as tall as its text.
fn flat_card<'a>(
    t: &GraphTask,
    s: WorkState,
    line: String,
    alpha: f32,
    selected: bool,
) -> Element<'a, Msg> {
    let (edge, ring) = card_frame(s, selected);
    button(card_body(t, s, line, alpha, false))
        .padding(0)
        .width(Length::Fill)
        .style(move |_: &Theme, status| {
            let pal = theme::palette();
            button::Style {
                background: Some(
                    fade(
                        if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                            pal.hover
                        } else {
                            pal.raised
                        },
                        alpha,
                    )
                    .into(),
                ),
                text_color: pal.ink,
                border: Border {
                    color: fade(edge, alpha),
                    width: 1.0,
                    radius: theme::skin().radius.into(),
                },
                shadow: ring.map_or(Shadow::default(), |color| Shadow {
                    color: fade(color, alpha),
                    ..print()
                }),
                ..Default::default()
            }
        })
        .on_press(Msg::Select(Some(t.id.clone())))
        .into()
}

/// While the project plans, its stages are the columns: that is where the work is.
fn planning<'a>(p: &'a Project) -> Element<'a, Msg> {
    let pal = theme::palette();
    let at = model::stage_index(&p.stage);
    let doc =
        |kind: String, title: String, meta: String, tone: Option<Color>| -> Element<'a, Msg> {
            let edge = if tone == Some(pal.signal) {
                pal.signal
            } else {
                pal.rule
            };
            let mut lines = column![].spacing(2);
            if !kind.is_empty() {
                lines = lines.push(ui::mono(kind, 10.5, tone.unwrap_or(pal.ink_3)));
            }
            container(
                lines
                    .push(
                        text(title)
                            .font(ui::SANS_SEMIBOLD)
                            .size(13.0)
                            .color(pal.ink)
                            .wrapping(Wrapping::WordOrGlyph),
                    )
                    .push(ui::sans(meta, 11.5, pal.ink_3).wrapping(Wrapping::WordOrGlyph)),
            )
            .padding(Padding {
                top: 8.0,
                right: 12.0,
                bottom: 8.0,
                left: 12.0,
            })
            .width(Length::Fill)
            .style(move |_: &Theme| container::Style {
                background: Some(theme::palette().sheet_2.into()),
                border: Border {
                    color: edge,
                    width: 1.0,
                    radius: theme::skin().radius.into(),
                },
                ..Default::default()
            })
            .into()
        };
    let docs_of = |kinds: &[&str]| -> Vec<Element<'a, Msg>> {
        p.docs
            .iter()
            .filter(|d| kinds.contains(&d.kind.as_str()))
            .map(|d| {
                doc(
                    d.kind.to_uppercase(),
                    d.title.clone(),
                    format!("{} · {}", d.by, ago(d.at_ms)),
                    None,
                )
            })
            .collect()
    };
    let col = |stage: &str, cards: Vec<Element<'a, Msg>>, idle: &str| -> Element<'a, Msg> {
        let i = model::stage_index(stage);
        let mut head = row![
            text(model::stage_label(stage))
                .font(ui::SANS_SEMIBOLD)
                .size(13.0)
                .color(pal.ink)
        ]
        .spacing(8)
        .align_y(Alignment::Center);
        if i == at {
            head = head.push(ui::badge("Now", ui::Tone::Signal));
        }
        // The stage's line, as its Plan card says it (the web's `.wk-note` under the title).
        let line = model::plan_cards(p)
            .into_iter()
            .find(|c| c.stage == stage)
            .map_or(String::new(), |c| c.line);
        let body: Element<'a, Msg> = if cards.is_empty() {
            ui::sans(idle.to_string(), 12.0, pal.ink_3)
                .wrapping(Wrapping::WordOrGlyph)
                .into()
        } else {
            column(cards).spacing(8).into()
        };
        stage_box(
            column![
                head,
                ui::sans(line, 12.0, pal.ink_3).wrapping(Wrapping::WordOrGlyph),
                body
            ]
            .spacing(10)
            .into(),
            i.cmp(&at),
        )
    };
    let critiques: Vec<Element<'a, Msg>> = p
        .critiques
        .iter()
        .map(|c| {
            let (kind, tone) = match c.verdict.as_str() {
                "approve" => ("APPROVES", pal.ok),
                "revise" => ("ASKS FOR A REVISION", pal.crit),
                _ => ("COMMENTS", pal.ink_3),
            };
            doc(
                kind.to_string(),
                c.by.clone(),
                format!("revision {} · {}", c.revision, ago(c.at_ms)),
                Some(tone),
            )
        })
        .collect();
    let approval = model::plan_cards(p).pop().map(|c| {
        doc(
            String::new(),
            c.title.clone(),
            c.line.clone(),
            (c.state == PlanState::You).then_some(pal.signal),
        )
    });
    // The tasks stand under the plan, not in it (the web's `section.wk-note`).
    let tasks_note = ui::sans(
        "Tasks: Posted from the plan’s tasks list once it is approved, each after the tasks it names.",
        12.0,
        pal.ink_3,
    )
    .wrapping(Wrapping::WordOrGlyph);
    column![
        row![
            col(
                "brief",
                docs_of(&["brief"]),
                "The orchestrator writes the brief."
            ),
            col(
                "research",
                docs_of(&["research"]),
                "Researchers attach what they find."
            ),
            col(
                "proposal",
                docs_of(&["proposal", "design", "spec", "tasks", "revision"]),
                "The planner writes the proposal, the design and the tasks list."
            ),
            col(
                "critique",
                critiques,
                "Agents other than the author critique the plan."
            ),
            col(
                "approval",
                approval.into_iter().collect(),
                "The mayor approves."
            ),
        ]
        .spacing(10),
        tasks_note,
    ]
    .spacing(8)
    .into()
}

/// A planning stage's column: the current one in ink with the print shadow, a later one in the faint rule.
fn stage_box<'a>(body: Element<'a, Msg>, place: std::cmp::Ordering) -> Element<'a, Msg> {
    container(body)
        .padding(12)
        .width(Length::FillPortion(1))
        .height(Length::Shrink)
        .style(move |_: &Theme| {
            let pal = theme::palette();
            container::Style {
                background: Some(pal.sheet.into()),
                border: Border {
                    color: match place {
                        std::cmp::Ordering::Equal => pal.signal,
                        std::cmp::Ordering::Less => pal.ok,
                        std::cmp::Ordering::Greater => pal.rule_faint,
                    },
                    width: 1.0,
                    radius: theme::skin().radius_lg.into(),
                },
                shadow: Shadow::default(),
                ..Default::default()
            }
        })
        .into()
}

/// "5m ago", as the other screens say it.
fn ago(ms: i64) -> String {
    let s = (now_ms() - ms).max(0) / 1000;
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86_399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
}

// ---- The drawer: the task's lifecycle --------------------------------------------------------------------------------

/// One step of the lifecycle: its mark, what it says (text and links), and its tone.
enum Tone {
    Plain,
    Done,
    Now,
    You,
    Bad,
}

fn drawer<'a>(state: &'a State, g: &model::Graph) -> Option<Element<'a, Msg>> {
    let id = state.selected.as_deref()?;
    let t = g.tasks.iter().find(|x| x.id == id)?;
    let pal = theme::palette();
    let s = state_of(state, t);
    let by_id: HashMap<&str, &GraphTask> = g.tasks.iter().map(|x| (x.id.as_str(), x)).collect();
    let detail = state.detail.as_ref().filter(|d| d.id == t.id);
    let mut steps: Vec<Element<'a, Msg>> = Vec::new();
    let mut step = |mark: &str, body: Element<'a, Msg>, tone: Tone| {
        let color = match tone {
            Tone::Plain => pal.ink_2,
            Tone::Done => pal.ink,
            Tone::Now => pal.info,
            Tone::You => pal.signal_ink,
            Tone::Bad => pal.crit,
        };
        steps.push(
            row![
                container(ui::sans(mark.to_string(), 12.0, color))
                    .width(Length::Fixed(16.0))
                    .align_x(Alignment::Center),
                body,
            ]
            .spacing(6)
            .into(),
        );
    };
    let say = |words: String, color: Color| -> Element<'a, Msg> {
        ui::sans(words, 12.0, color)
            .wrapping(Wrapping::WordOrGlyph)
            .into()
    };
    step(
        "·",
        say(
            detail.map_or("Posted".to_string(), |d| {
                format!("Posted {}", ago(d.created_at))
            }),
            pal.ink,
        ),
        Tone::Done,
    );
    let after = model::upstream_of(&t.id, &g.edges);
    if !after.is_empty() {
        let links: Vec<Element<'a, Msg>> = after
            .iter()
            .map(|up| {
                let task = by_id.get(up.as_str());
                let label = match task {
                    Some(u) => format!(
                        "{up} {} ({})",
                        ui::ellipsize(&model::title_of(&u.goal), 34),
                        state_of(state, u).label().to_lowercase()
                    ),
                    None => up.clone(),
                };
                drawer_link(label, Msg::Select(Some(up.clone())))
            })
            .collect();
        let done = after
            .iter()
            .all(|up| by_id.get(up.as_str()).is_some_and(|u| u.state == "done"));
        step(
            "↳",
            column![say("After".into(), pal.ink_2), column(links).spacing(4)]
                .spacing(4)
                .into(),
            if done { Tone::Done } else { Tone::Plain },
        );
    }
    let now = now_ms();
    if let Some(cid) = t.agent_cid {
        step(
            "→",
            row![
                say("Claimed by".into(), pal.ink),
                drawer_link(name_of(state, cid), Msg::Go(Id::Agent(cid)))
            ]
            .spacing(4)
            .into(),
            Tone::Done,
        );
        if matches!(s, WorkState::Working | WorkState::NeedsYou) {
            let title = |id: &str| {
                by_id
                    .get(id)
                    .map_or(id.to_string(), |u| model::title_of(&u.goal))
            };
            let hub_state = |id: &str| by_id.get(id).map(|u| u.state.clone());
            let tasks = ledger(state);
            let ready = |id: &str| ready_at(&tasks, id);
            let name = |cid: u64| name_of(state, cid);
            let agent_step = |cid: u64| step_of(state, cid);
            let ctx = LineContext {
                edges: &g.edges,
                state_of: &hub_state,
                task_name: &title,
                ready_at: &ready,
                name_of: &name,
                step_of: &agent_step,
                now,
            };
            let you = s == WorkState::NeedsYou;
            step(
                "●",
                say(
                    model::card_line(t, s, &ctx),
                    if you { pal.signal_ink } else { pal.info },
                ),
                if you { Tone::You } else { Tone::Now },
            );
            if you {
                step(
                    "!",
                    drawer_link("Decide in Approvals".into(), Msg::Go(Id::Approvals)),
                    Tone::You,
                );
            }
        }
    } else if s == WorkState::Ready {
        let waited = ready_at(&ledger(state), &t.id)
            .map(|at| format!(" · {}", model::short_span(now - at)))
            .unwrap_or_default();
        step(
            "○",
            say(
                format!(
                    "Waits for a free {}{waited}",
                    model::claimant(t.role.as_deref())
                ),
                pal.info,
            ),
            Tone::Now,
        );
    }
    if s == WorkState::Review {
        let body: Element<'a, Msg> = match t.reviewer_cid {
            Some(cid) => row![
                say("In review by".into(), pal.info),
                drawer_link(name_of(state, cid), Msg::Go(Id::Agent(cid)))
            ]
            .spacing(4)
            .into(),
            None => say("Waits for a reviewer".into(), pal.info),
        };
        step("●", body, Tone::Now);
    }
    if s.ended() {
        let note = detail
            .and_then(|d| d.note.clone().or_else(|| d.outcome.clone()))
            .and_then(|n| n.lines().next().map(str::to_string))
            .map_or(String::new(), |n| format!(" · {n}"));
        let done = s == WorkState::Done;
        step(
            if done { "✓" } else { "×" },
            say(
                format!("{}{note}", s.label()),
                if done { pal.ink } else { pal.crit },
            ),
            if done { Tone::Done } else { Tone::Bad },
        );
    }
    if s == WorkState::Blocked {
        step(
            "×",
            say(
                format!(
                    "Blocked: {} did not finish. The orchestrator relinks it, posts a replacement, or withdraws it.",
                    t.blocked_by.join(", ")
                ),
                pal.crit,
            ),
            Tone::Bad,
        );
    }
    let close = button(ui::icon("x", 14.0, pal.ink_2))
        .padding(4)
        .style(button::text)
        .on_press(Msg::Select(None));
    let mut badges = row![ui::badge_dot::<Msg>(s.label(), tone(s))]
        .spacing(10)
        .align_y(Alignment::Center);
    if let Some(role) = t.role.as_deref() {
        badges = badges.push(ui::badge::<Msg>(&roles::label(role), ui::Tone::Muted));
    }
    let body = column![
        row![
            ui::mono(t.id.clone(), 11.0, pal.ink_3),
            Space::new().width(Length::Fill),
            close
        ]
        .align_y(Alignment::Center),
        text(model::title_of(&t.goal))
            .font(ui::SANS_SEMIBOLD)
            .size(theme::TYPE_SECTION)
            .color(pal.ink)
            .wrapping(Wrapping::WordOrGlyph),
        badges,
        column(steps).spacing(8),
        ui::link("Open the task", Msg::Go(Id::Task(t.id.clone())), false),
    ]
    .spacing(12);
    Some(
        container(body)
            .padding(16)
            .width(Length::Fixed(DRAWER_W))
            .style(theme::sheet)
            .into(),
    )
}

/// A link in the drawer: info-blue and underlined, as the web's `.wk-link`.
fn drawer_link<'a>(label: String, msg: Msg) -> Element<'a, Msg> {
    let underline = container(Space::new().height(1.0))
        .width(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(fade(theme::palette().info, 0.6).into()),
            ..Default::default()
        });
    button(
        column![
            text(label).size(12.0).wrapping(Wrapping::WordOrGlyph),
            underline
        ]
        .spacing(1),
    )
    .padding(0)
    .style(|_: &Theme, status| button::Style {
        background: None,
        text_color: if matches!(status, button::Status::Hovered | button::Status::Pressed) {
            theme::palette().signal_ink
        } else {
            theme::palette().info
        },
        ..Default::default()
    })
    .on_press(msg)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The web's `CARD` (apps/web/src/views/work.ts), read from the source as JSON. Its values are the web's named
    /// sizes (`const CW = 232;`), so each name is looked up in the same file.
    fn web_card() -> String {
        let source = include_str!("../../../../web/src/views/work.ts");
        let number = |name: &str| -> f64 {
            let prefix = format!("const {name} = ");
            source
                .lines()
                .find_map(|l| l.trim_start().strip_prefix(&prefix))
                .unwrap_or_else(|| panic!("work.ts declares {name}"))
                .trim_end_matches(';')
                .trim()
                .parse()
                .expect("a number")
        };
        let line = source
            .lines()
            .find(|l| l.trim_start().starts_with("const CARD = {"))
            .expect("work.ts declares CARD");
        let body = &line[line.find('{').expect("an object")..=line.rfind('}').expect("closed")];
        let mut json = String::new();
        for part in body.trim_matches(|c| c == '{' || c == '}').split(',') {
            let (key, value) = part.split_once(':').expect("key: value");
            let value = value.trim();
            let value = value.parse::<f64>().unwrap_or_else(|_| number(value));
            if !json.is_empty() {
                json.push(',');
            }
            json.push_str(&format!("\"{}\":{value}", key.trim()));
        }
        format!("{{{json}}}")
    }

    #[test]
    fn the_desk_sizes_are_the_webs() {
        let web: apz_graph::Options =
            serde_json::from_str(&web_card()).expect("the web's CARD reads as options");
        assert_eq!(web, CARD);
    }

    /// The fixtures in assets/graphs, laid out as the desktop lays them out and as the web's binding does (the wasm's
    /// JSON layer, run natively) with the web's options: the same cards and the same routes.
    #[test]
    fn the_desktop_and_the_web_lay_the_fixtures_out_alike() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/graphs");
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).expect("the fixtures") {
            let path = entry.expect("a fixture").path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let fixture: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).expect("a fixture reads"))
                    .expect("a fixture parses");
            let graph = fixture["graph"].to_string();
            let web =
                apz_graph_web::model::layout(&graph, &web_card()).expect("the web lays it out");
            let input: apz_graph::Graph =
                serde_json::from_value(fixture["graph"].clone()).expect("the read model reads");
            let desk = apz_graph::layout(&input, &CARD);
            let web: apz_graph::Layout =
                serde_json::from_str(&web).expect("the web's layout reads");
            assert_eq!(desk, web, "{}", path.display());
            checked += 1;
        }
        assert!(checked > 0, "no fixtures in {}", dir.display());
    }

    fn graph(edges: &[(&str, &str)], ids: &[&str]) -> model::Graph {
        serde_json::from_value(json!({
            "tasks": ids.iter().map(|id| json!({ "id": id, "goal": id, "state": "open" })).collect::<Vec<_>>(),
            "edges": edges.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>(),
        }))
        .expect("a graph reads")
    }

    #[test]
    fn a_layout_is_kept_while_its_tasks_and_links_are_the_same() {
        let mut state = State {
            current: "pr1".into(),
            graph: Some(graph(&[("t1", "t2")], &["t1", "t2"])),
            ..Default::default()
        };
        keep_layout(&mut state);
        let first = state.layout.clone().expect("a linked graph is laid out");
        // A new read of the same graph keeps the layout (the key is the same); another link lays it out again.
        state.graph = Some(graph(&[("t1", "t2")], &["t1", "t2"]));
        keep_layout(&mut state);
        assert_eq!(state.layout.as_ref().map(|(k, _)| k), Some(&first.0));
        state.graph = Some(graph(&[("t1", "t2"), ("t2", "t3")], &["t1", "t2", "t3"]));
        keep_layout(&mut state);
        assert_ne!(state.layout.as_ref().map(|(k, _)| k), Some(&first.0));
        // No links: state columns, no layout.
        state.graph = Some(graph(&[], &["t1"]));
        keep_layout(&mut state);
        assert!(state.layout.is_none());
    }

    #[test]
    fn a_step_event_is_newer_than_the_roster() {
        let mut state = State {
            agents: vec![Agent {
                cid: 7,
                name: "Ada".into(),
                step: Some(Step {
                    stage: "thinking".into(),
                    tool: None,
                    since_ms: 1,
                }),
            }],
            ..Default::default()
        };
        assert_eq!(step_of(&state, 7).map(|s| s.stage), Some("thinking".into()));
        state.steps.insert(
            7,
            Some(Step {
                stage: "approval".into(),
                tool: Some("plaza_pay".into()),
                since_ms: 2,
            }),
        );
        assert_eq!(step_of(&state, 7).map(|s| s.stage), Some("approval".into()));
        state.steps.insert(7, None);
        assert_eq!(step_of(&state, 7), None);
        assert_eq!(name_of(&state, 7), "Ada");
        assert_eq!(name_of(&state, 8), "agent 8");
    }
}
