//! The Overview screen (parity with the web console's `overview` view, in the prototype's bare page): the
//! orchestrator's composer under the plaza's question, the three ways in, the figures, what waits on the owner
//! beside the live activity, the projects in progress and the fleet. Under a project it is that project's: its
//! agents, its tasks, its stage and budget, its activity.
//!
//! The web view is pushed the transcript bus. A screen has no event stream here, so the feed is
//! polled: every load reads each agent's transcript after the last entry seen.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::alignment::{Horizontal, Vertical};
use iced::keyboard::{Key, key::Named};
use iced::widget::text::{LineHeight, Wrapping};
use iced::widget::{Space, button, column, container, row, space, text, text_editor};
use iced::{Background, Border, Color, Element, Font, Length, Padding, Task, Theme};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::Id;
use super::approvals::{self, Approval, Proposal};
use super::work_model::{self as model};
use crate::console::{Ctx, chat_tools, client, roles, ui};
use crate::theme::{self, TYPE_META, TYPE_PROSE, TYPE_UI};

const FEED_MAX: usize = 40;
const FLEET_COLS: usize = 4;
const SANS_MEDIUM: Font = ui::SANS_MEDIUM;
const SANS_SEMIBOLD: Font = ui::SANS_SEMIBOLD;

#[derive(Debug, Default)]
pub struct State {
    /// The orchestrator composer's model chip and context bar (its orchestrator's work).
    pub chat_tools: chat_tools::ChatTools,
    pub error: Option<String>,
    overview: Option<Overview>,
    agents: Vec<Agent>,
    feed: Vec<Item>,
    /// The projects and the ledger, for the projects' lines; None when the hub would not say.
    projects: Option<(Vec<model::Project>, Vec<model::Task>)>,
    /// The approvals and the agents the orchestrator proposes: what waits on the owner.
    approvals: Vec<Approval>,
    proposals: Vec<Proposal>,
    /// The agents' steps as `agent_step` events bring them, newer than the roster's until the next read.
    steps: HashMap<u64, Option<model::Step>>,
    cursors: HashMap<u64, u64>,
    goal: text_editor::Content,
    sending: bool,
    chat_error: Option<String>,
    /// A load is in flight; hub events that arrive meanwhile only mark `again`.
    loading: bool,
    again: bool,
    /// Under a project, the credits left in its own budget (None: it is funded from the plaza's).
    budget: Option<i64>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    /// Open the New project dialog (the three ways in).
    NewProject,
    /// One poll of the hub: the overview, the fleet, the waiting list and the new transcript entries.
    Loaded(Result<Snapshot, String>),
    /// An edit to the orchestrator goal.
    Goal(text_editor::Action),
    /// Send the goal to the orchestrator.
    Send,
    /// The chat message was accepted or refused.
    Sent(Result<Value, String>),
    /// A hub event: the roster or the transcript changed.
    Event(crate::console::events::Event),
    /// The orchestrator composer's model chip and context bar.
    ChatTools(chat_tools::ChatToolsMsg),
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    overview: Overview,
    agents: Vec<Agent>,
    feed: Vec<(u64, Vec<Entry>)>,
    projects: Option<(Vec<model::Project>, Vec<model::Task>)>,
    approvals: Vec<Approval>,
    proposals: Vec<Proposal>,
    budget: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
struct Overview {
    agents: Counts,
    tasks: HashMap<String, u64>,
    default_model: String,
    orchestrator_model: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Counts {
    total: u64,
    working: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct Agent {
    cid: u64,
    role: String,
    role_label: String,
    tier: String,
    status: String,
    model: String,
    work: Option<Work>,
    /// What it is doing now: a step waiting on an approval makes its task the owner's square.
    #[serde(default)]
    step: Option<model::Step>,
    /// The project it works on (None: the bench, or the orchestrator).
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    kind: String,
}

impl Agent {
    /// Shown under `project` (None: All projects): its member, or the orchestrator.
    fn in_scope(&self, project: Option<&str>) -> bool {
        project.is_none_or(|p| self.project.as_deref() == Some(p) || self.tier == "orchestrator")
    }

    /// On no project, and neither the orchestrator nor a model-only agent.
    fn on_bench(&self) -> bool {
        self.project.as_deref().is_none_or(str::is_empty)
            && self.tier != "orchestrator"
            && self.kind != "model"
    }
}

#[derive(Debug, Clone, Deserialize)]
struct Work {
    prompt: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Entry {
    seq: u64,
    at_ms: u64,
    kind: String,
    data: Map<String, Value>,
}

/// One line of the activity feed.
#[derive(Debug, Clone)]
struct Item {
    at_ms: u64,
    cid: u64,
    line: String,
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// The hub's events while the screen is open: the web refreshes the fleet on `agent` and `work`
/// frames and the feed on `transcript` frames.
pub fn subscription(ctx: &Ctx) -> iced::Subscription<Msg> {
    crate::console::events::subscription(&ctx.client).map(Msg::Event)
}

/// Loads the screen's data (called when it opens, on every poll tick and on a hub event).
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    state.loading = true;
    let client = ctx.client.clone();
    let cursors = state.cursors.clone();
    let scope = ctx.project.clone();
    client::task(
        move || {
            let overview: Overview = client.get("/v1/overview")?;
            let agents: Vec<Agent> = client.get("/v1/agents")?;
            let mut feed = Vec::with_capacity(agents.len());
            // Under a project, the activity of its agents (and the orchestrator) only.
            for a in agents.iter().filter(|a| a.in_scope(scope.as_deref())) {
                let path = match cursors.get(&a.cid) {
                    Some(after) => {
                        format!("/v1/agents/{}/transcript?after={after}&limit=200", a.cid)
                    }
                    None => format!("/v1/agents/{}/transcript?after=0&limit=1000", a.cid),
                };
                let entries: Vec<Entry> = client.get(&path)?;
                feed.push((a.cid, entries));
            }
            // What waits on the owner: a list that could not be read keeps the last one the owner saw.
            let approvals = client
                .get::<Value>("/v1/approvals")
                .map(|v| approvals::list(&v, Approval::from_value))
                .unwrap_or_default();
            let proposals = client
                .get::<Value>("/v1/agent-proposals")
                .map(|v| approvals::list(&v, Proposal::from_value))
                .unwrap_or_default();
            let budget = scope.as_deref().and_then(|p| {
                let b: Value = client.get(&format!("/v1/projects/{p}/budget")).ok()?;
                b["own"]
                    .as_bool()
                    .unwrap_or(false)
                    .then(|| b["balance"].as_i64().unwrap_or(0))
            });
            Ok(Snapshot {
                overview,
                agents,
                feed,
                projects: read_projects(&client, scope.as_deref()),
                approvals,
                proposals,
                budget,
            })
        },
        Msg::Loaded,
    )
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) | Msg::NewProject => {}
        Msg::Loaded(Ok(snap)) => {
            for (cid, entries) in snap.feed {
                for e in entries {
                    let cursor = state.cursors.entry(cid).or_insert(0);
                    *cursor = (*cursor).max(e.seq);
                    if let Some(line) = describe(&e) {
                        state.feed.push(Item {
                            at_ms: e.at_ms,
                            cid,
                            line,
                        });
                    }
                }
            }
            state.feed.sort_by_key(|a| std::cmp::Reverse(a.at_ms));
            state.feed.truncate(FEED_MAX);
            state.overview = Some(snap.overview);
            state.agents = snap.agents;
            state.projects = snap.projects;
            state.approvals = snap.approvals;
            state.proposals = snap.proposals;
            state.budget = snap.budget;
            // The roster was read after the steps the events brought, so its own are as new.
            state.steps.clear();
            state.error = None;
            let tools = match state.agents.iter().find(|a| a.tier == "orchestrator") {
                Some(orch) if !state.chat_tools.is_loaded() => {
                    chat_tools::load(&ctx.client, chat_tools::Target::Agent(orch.cid))
                        .map(Msg::ChatTools)
                }
                _ => Task::none(),
            };
            return Task::batch([landed(state, ctx), tools]);
        }
        Msg::Loaded(Err(e)) => {
            state.overview = None;
            state.error = Some(e);
            return landed(state, ctx);
        }
        Msg::ChatTools(m) => {
            return chat_tools::update(&mut state.chat_tools, m, &ctx.client).map(Msg::ChatTools);
        }
        Msg::Event(event) => {
            if let Some(task) = state
                .chat_tools
                .on_event(&ctx.client, &event.kind, &event.data)
            {
                return task.map(Msg::ChatTools);
            }
            if event.kind == "agent_step" {
                if let Some(cid) = event.data.get("cid").and_then(Value::as_u64) {
                    let step = event
                        .data
                        .get("step")
                        .cloned()
                        .and_then(|s| serde_json::from_value::<model::Step>(s).ok());
                    state.steps.insert(cid, step);
                }
                return Task::none();
            }
            if !matches!(
                event.kind.as_str(),
                "agent"
                    | "work"
                    | "transcript"
                    | "project"
                    | "task"
                    | "approval"
                    | "agent_proposal"
            ) {
                return Task::none();
            }
            if state.loading {
                state.again = true;
                return Task::none();
            }
            return load(state, ctx);
        }
        Msg::Goal(action) => state.goal.perform(action),
        Msg::Send => {
            let text = state.goal.text().trim().to_string();
            let Some(cid) = state
                .agents
                .iter()
                .find(|a| a.tier == "orchestrator")
                .map(|a| a.cid)
            else {
                return Task::none();
            };
            if state.sending || text.is_empty() {
                return Task::none();
            }
            state.sending = true;
            state.chat_error = None;
            let client = ctx.client.clone();
            return client::task(
                move || {
                    client
                        .post::<Value>(&format!("/v1/agents/{cid}/chat"), &json!({ "text": text }))
                },
                Msg::Sent,
            );
        }
        Msg::Sent(Ok(_)) => {
            state.sending = false;
            state.goal = text_editor::Content::new();
            return state.chat_tools.refresh(&ctx.client).map(Msg::ChatTools);
        }
        Msg::Sent(Err(e)) => {
            state.sending = false;
            state.chat_error = Some(e);
        }
    }
    Task::none()
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

pub fn view<'a>(state: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    let scope = ctx.project.as_deref();
    let p = crate::theme::palette();
    // Under a project, the project's title is the page's question; the plaza's asks what it should work on.
    let title = match scope {
        Some(s) => state
            .projects
            .as_ref()
            .and_then(|(list, _)| list.iter().find(|x| x.id == s))
            .map_or(s.to_string(), |x| x.title.clone()),
        None => "What should the plaza work on?".to_string(),
    };
    let orch = state.agents.iter().find(|a| a.tier == "orchestrator");
    let sub = match orch {
        Some(o) => format!(
            "{} plans the work and delegates it to the fleet · {} · Ctrl+Enter to send",
            roles::agent_label(&o.role, o.cid),
            o.model
        ),
        None => String::new(),
    };
    let composer: Element<'a, Msg> = match orch {
        Some(o) => composer_box(state, o),
        None => boxed_card(ui::empty(
            "orchestrator",
            "No orchestrator connected",
            "Add the orchestrator role to the fleet to delegate work by goal.",
        )),
    };
    let hero = column![
        container(
            text(title)
                .font(SANS_SEMIBOLD)
                .size(26.0)
                .line_height(LineHeight::Relative(1.2))
                .color(p.ink)
        )
        .width(Length::Fill)
        .align_x(Horizontal::Center),
        container(text(sub).font(theme::SANS).size(TYPE_UI).color(p.ink_3))
            .width(Length::Fill)
            .align_x(Horizontal::Center),
    ]
    .spacing(8);

    let stats = match (&state.overview, scope) {
        (Some(_), Some(s)) => project_stats(state, s),
        (Some(o), None) => stat_row(o, state.agents.iter().filter(|a| a.on_bench()).count()),
        (None, _) => boxed_card(ui::empty(
            "alert",
            "apid is unreachable",
            "Start apid, then reload.",
        )),
    };

    let mut page = column![hero, composer, ways(), stats].spacing(16);
    if let Some(e) = &state.error {
        page = page.push(ui::error_line(e));
    }
    page = page.push(
        row![
            container(waiting(state, scope)).width(Length::FillPortion(1)),
            container(activity(state)).width(Length::FillPortion(1)),
        ]
        .spacing(16)
        .align_y(Vertical::Top),
    );
    page = page.push(projects(state));
    let fleet_agents: Vec<&Agent> = state.agents.iter().filter(|a| a.in_scope(scope)).collect();
    page = page.push(fleet(fleet_agents));

    // The page is centred at 820 wide and scrolls under the window; its gutters are the web's `.home` padding.
    container(ui::scroll(
        container(page)
            .max_width(820.0)
            .width(Length::Fill)
            .padding(Padding {
                top: 48.0,
                right: 28.0,
                bottom: 48.0,
                left: 28.0,
            }),
    ))
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// The projects and the ledger, for the Projects card; None when the hub would not say. Under a project, that
/// project and its tasks.
fn read_projects(
    client: &client::Client,
    scope: Option<&str>,
) -> Option<(Vec<model::Project>, Vec<model::Task>)> {
    #[derive(Deserialize)]
    struct Tasks {
        tasks: Vec<model::Task>,
    }
    let mut projects: Vec<model::Project> = client.get("/v1/projects").ok()?;
    let path = match scope {
        Some(p) => format!("/v1/tasks?project={}", client::enc(p)),
        None => "/v1/tasks".to_string(),
    };
    let tasks = client.get::<Tasks>(&path).ok()?.tasks;
    if let Some(p) = scope {
        projects.retain(|x| x.id == p);
    }
    Some((projects, tasks))
}

/// Under a project, its figures: its agents, its open tasks, its stage, its budget.
fn project_stats<'a>(state: &'a State, project: &str) -> Element<'a, Msg> {
    let team: Vec<&Agent> = state
        .agents
        .iter()
        .filter(|a| a.project.as_deref() == Some(project))
        .collect();
    let working = team.iter().filter(|a| a.status == "working").count();
    let (tasks, p) = match &state.projects {
        Some((list, tasks)) => (tasks.as_slice(), list.iter().find(|x| x.id == project)),
        None => (&[][..], None),
    };
    let open = tasks
        .iter()
        .filter(|t| matches!(t.state.as_str(), "open" | "claimed"))
        .count();
    let done = tasks.iter().filter(|t| t.state == "done").count();
    let (stage, life) = match p {
        Some(p) => (
            model::stage_label(&p.stage),
            if p.paused {
                "paused"
            } else if p.archived {
                "archived"
            } else {
                "in progress"
            },
        ),
        None => ("—".to_string(), ""),
    };
    let (budget, budget_sub) = match state.budget {
        Some(b) => (ui::thousands(b), "credits left"),
        None => ("—".to_string(), "from the plaza"),
    };
    row![
        stat(
            "Agents",
            figure(team.len().to_string()),
            format!("{working} working")
        ),
        stat(
            "Open tasks",
            figure(open.to_string()),
            format!("{done} done")
        ),
        stat("Stage", figure(stage), life.to_string()),
        stat("Budget", figure(budget), budget_sub.to_string()),
    ]
    .spacing(16)
    .into()
}

fn stat_row<'a>(o: &Overview, bench: usize) -> Element<'a, Msg> {
    let open =
        o.tasks.get("open").copied().unwrap_or(0) + o.tasks.get("claimed").copied().unwrap_or(0);
    let done = o.tasks.get("done").copied().unwrap_or(0);
    row![
        stat(
            "Agents",
            figure(o.agents.total.to_string()),
            format!("{} working · {bench} on the bench", o.agents.working)
        ),
        stat(
            "Open tasks",
            figure(open.to_string()),
            format!("{done} done")
        ),
        model_stat("Worker model", &o.default_model),
        model_stat("Orchestrator model", &o.orchestrator_model),
    ]
    .spacing(16)
    .into()
}

/// A model's stat: its name in mono, its provider under it (the web's `modelStat`).
fn model_stat<'a>(label: &str, model: &str) -> Element<'a, Msg> {
    let s = model.to_string();
    let (name, provider) = match s.find('/') {
        Some(i) => (
            s.rsplit('/').next().unwrap_or("").to_string(),
            s[..i].to_string(),
        ),
        None => (s.clone(), String::new()),
    };
    let name = if name.is_empty() {
        "—".to_string()
    } else {
        name
    };
    stat(
        label,
        text(name)
            .font(theme::MONO)
            .size(15.0)
            .wrapping(Wrapping::None)
            .color(crate::theme::palette().ink)
            .into(),
        provider,
    )
}

/// A stat's figure: semibold 19, one line.
fn figure<'a>(value: String) -> Element<'a, Msg> {
    text(value)
        .font(SANS_SEMIBOLD)
        .size(19.0)
        .wrapping(Wrapping::None)
        .color(crate::theme::palette().ink)
        .into()
}

/// A stat tile, as the web's `.stat`: the label, the figure, a sub-line; a raised box with a rule.
fn stat<'a>(label: &str, value: Element<'a, Msg>, sub: String) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    container(
        column![
            text(label.to_string())
                .font(theme::SANS)
                .size(TYPE_META)
                .color(p.ink_3)
                .wrapping(Wrapping::None),
            value,
            text(sub)
                .font(theme::SANS)
                .size(TYPE_META)
                .color(p.ink_3)
                .wrapping(Wrapping::None),
        ]
        .spacing(4),
    )
    .padding(Padding {
        top: 14.0,
        right: 16.0,
        bottom: 14.0,
        left: 16.0,
    })
    .width(Length::FillPortion(1))
    .clip(true)
    .style(boxed)
    .into()
}

/// The composer to the orchestrator, as the web's `.composer`: the message, and its bar with the agent it goes to,
/// the transcript link and Send.
fn composer_box<'a>(state: &'a State, orch: &'a Agent) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let (label, tone) = match orch.status.as_str() {
        "working" => ("Working", p.info),
        "idle" => ("Idle", p.ok),
        "asleep" => ("Asleep", p.ink_3),
        "dozing" => ("Asleep (light)", p.ink_3),
        _ => ("Offline", p.ink_3),
    };
    let chip = container(chat_tools::chip(&state.chat_tools).map(Msg::ChatTools))
        .width(Length::Fixed(360.0));
    let pick = row![
        ui::dot::<Msg>(tone),
        text(roles::agent_label(&orch.role, orch.cid))
            .font(SANS_MEDIUM)
            .size(TYPE_UI)
            .color(p.ink)
            .wrapping(Wrapping::None),
        text(label)
            .font(theme::SANS)
            .size(TYPE_META)
            .color(p.ink_3)
            .wrapping(Wrapping::None),
    ]
    .spacing(8)
    .align_y(Vertical::Center);

    let goal = text_editor(&state.goal)
        .placeholder("Tell the orchestrator what you want done…")
        .on_action(Msg::Goal)
        .key_binding(|press| {
            if matches!(press.key, Key::Named(Named::Enter))
                && (press.modifiers.control() || press.modifiers.logo())
            {
                return Some(text_editor::Binding::Custom(Msg::Send));
            }
            text_editor::Binding::from_key_press(press)
        })
        .font(theme::SANS)
        .size(TYPE_PROSE)
        .padding(Padding {
            top: 14.0,
            right: 16.0,
            bottom: 6.0,
            left: 16.0,
        })
        .height(Length::Fixed(84.0))
        .style(|_: &Theme, _| text_editor::Style {
            background: Background::Color(Color::TRANSPARENT),
            border: Border::default(),
            placeholder: crate::theme::palette().ink_3,
            value: crate::theme::palette().ink,
            selection: crate::theme::palette().signal,
        });

    let bar = row![
        pick,
        chip,
        space::horizontal(),
        ui::btn_sm(
            "Open transcript",
            Some("scroll-text"),
            ui::Kind::Ghost,
            Some(Msg::Go(Id::Agent(orch.cid)))
        ),
        ui::btn_sm(
            "Send",
            Some("arrow-up"),
            ui::Kind::Primary,
            (!state.sending).then_some(Msg::Send)
        ),
    ]
    .spacing(8)
    .align_y(Vertical::Center);

    let context = container(chat_tools::bar(&state.chat_tools).map(Msg::ChatTools))
        .padding(Padding {
            top: 10.0,
            right: 16.0,
            bottom: 0.0,
            left: 16.0,
        })
        .width(Length::Fill);
    let mut body = column![
        context,
        goal,
        container(bar)
            .padding(Padding {
                top: 6.0,
                right: 10.0,
                bottom: 8.0,
                left: 10.0,
            })
            .width(Length::Fill),
    ]
    .spacing(0);
    if let Some(e) = chat_tools::notice(&state.chat_tools) {
        body = body.push(container(e.map(Msg::ChatTools)).padding(Padding {
            top: 0.0,
            right: 16.0,
            bottom: 10.0,
            left: 16.0,
        }));
    }
    if let Some(e) = &state.chat_error {
        body = body.push(container(ui::error_line(e)).padding(Padding {
            top: 0.0,
            right: 16.0,
            bottom: 10.0,
            left: 16.0,
        }));
    }
    container(body)
        .width(Length::Fill)
        .clip(true)
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().raised.into()),
            border: Border {
                color: crate::theme::palette().rule_strong,
                width: 1.0,
                radius: 18.0.into(),
            },
            shadow: crate::theme::skin().shadow_card,
            ..Default::default()
        })
        .into()
}

/// The three ways in, as the web's `.home-ways`: one raised card of rows; each opens the New project dialog.
fn ways<'a>() -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let items = [
        ("messages-square", "Start a project by talking it through"),
        ("folder-plus", "Open a project directly"),
        ("git-pull-request-arrow", "Bring in a project that exists"),
    ];
    let mut list = column![].spacing(0);
    for (i, (icon, label)) in items.into_iter().enumerate() {
        if i > 0 {
            list = list.push(rule(p.rule));
        }
        let inner = row![
            ui::icon::<Msg>(icon, 15.0, p.ink_2),
            text(label)
                .font(theme::SANS)
                .size(TYPE_UI)
                .color(p.ink_2)
                .wrapping(Wrapping::None),
            space::horizontal(),
            text("New project")
                .font(theme::SANS)
                .size(TYPE_META)
                .color(p.ink_3)
                .wrapping(Wrapping::None),
            ui::icon::<Msg>("arrow-right", 15.0, p.ink_3),
        ]
        .spacing(12)
        .align_y(Vertical::Center);
        list = list.push(
            button(container(inner).center_y(Length::Fill))
                .on_press(Msg::NewProject)
                .padding([0.0, 16.0])
                .height(Length::Fixed(44.0))
                .width(Length::Fill)
                .style(|_: &Theme, status: button::Status| {
                    let p = crate::theme::palette();
                    let hovered =
                        matches!(status, button::Status::Hovered | button::Status::Pressed);
                    button::Style {
                        background: hovered.then_some(Background::Color(p.hover)),
                        text_color: if hovered { p.ink } else { p.ink_2 },
                        border: Border::default(),
                        ..Default::default()
                    }
                }),
        );
    }
    container(list)
        .width(Length::Fill)
        .clip(true)
        .style(|_: &Theme| container::Style {
            background: Some(crate::theme::palette().raised.into()),
            border: Border {
                color: crate::theme::palette().rule,
                width: 1.0,
                radius: crate::theme::skin().radius_lg.into(),
            },
            shadow: crate::theme::skin().shadow_card,
            ..Default::default()
        })
        .into()
}

/// What waits on the owner: the pending approvals and the agents proposed, under a project only its own.
fn waiting<'a>(state: &'a State, scope: Option<&str>) -> Element<'a, Msg> {
    let shown = |a: &Approval| -> bool {
        if scope.is_none() || a.cid == 0 {
            return true;
        }
        state
            .agents
            .iter()
            .find(|x| x.cid == a.cid)
            .is_none_or(|agent| agent.in_scope(scope))
    };
    let mut list: Vec<&Approval> = state
        .approvals
        .iter()
        .filter(|a| a.state == "pending" && shown(a))
        .collect();
    list.sort_by_key(|a| a.created_ms);
    let props: Vec<&Proposal> = state
        .proposals
        .iter()
        .filter(|p| p.state == "pending")
        .collect();
    let n = list.len() + props.len();

    // The rows are ruled apart, the first one unruled (the web's `.ov-wait a`).
    let mut rows = column![].spacing(0);
    let mut lines: Vec<(&str, String, String)> = Vec::new();
    for a in &list {
        let who = label_of(state, a.cid);
        lines.push((
            kind_icon(&a.kind),
            wait_title(a),
            format!("{who} · {}", approvals::ago(a.created_ms)),
        ));
    }
    for p in &props {
        let title = if p.title.is_empty() {
            "Proposed agent".to_string()
        } else {
            p.title.clone()
        };
        lines.push((
            "user-plus",
            title,
            format!(
                "{} · {}",
                label_of(state, p.by_cid),
                approvals::ago(p.created_ms)
            ),
        ));
    }
    for (i, (icon, title, sub)) in lines.into_iter().enumerate() {
        if i > 0 {
            rows = rows.push(rule(crate::theme::palette().rule));
        }
        rows = rows.push(wait_row(icon, title, sub));
    }
    let body: Element<'a, Msg> = if n == 0 {
        ui::empty(
            "approval",
            "Nothing is waiting",
            "Agents ask here before acts that need your approval, and the orchestrator proposes new agents here.",
        )
    } else {
        rows.into()
    };
    ov_card(
        vec![
            head_title(&format!("Waiting on you · {n}")),
            space::horizontal().into(),
            ui::btn_sm(
                "Approvals",
                None,
                ui::Kind::Ghost,
                Some(Msg::Go(Id::Approvals)),
            ),
        ],
        body,
        true,
    )
}

/// One row of the waiting list: an icon, its line (clipped), and who asked and when.
fn wait_row<'a>(icon: &str, title: String, sub: String) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let inner = row![
        ui::icon::<Msg>(icon, 15.0, p.ink_2),
        container(
            text(title)
                .font(theme::SANS)
                .size(TYPE_UI)
                .color(p.ink)
                .wrapping(Wrapping::None),
        )
        .width(Length::Fill)
        .clip(true),
        text(sub)
            .font(theme::SANS)
            .size(TYPE_META)
            .color(p.ink_3)
            .wrapping(Wrapping::None),
    ]
    .spacing(10)
    .align_y(Vertical::Center);
    button(container(inner).center_y(Length::Fill))
        .on_press(Msg::Go(Id::Approvals))
        .padding([0.0, 16.0])
        .height(Length::Fixed(44.0))
        .width(Length::Fill)
        .style(row_hover)
        .into()
}

/// The line of a waiting approval: its goal, its payment, or the tool call.
fn wait_title(a: &Approval) -> String {
    let goal = a.detail.get("goal").and_then(Value::as_str);
    let task = a.detail.get("task").and_then(Value::as_str);
    match a.kind.as_str() {
        "review" => goal
            .map(str::to_string)
            .unwrap_or_else(|| format!("Review of {}", task.unwrap_or(""))),
        "payment" => format!("Payment awaiting approval: {}", a.subject),
        "deploy" => a.subject.clone(),
        "artifact.share" => format!("Share {}", a.subject),
        _ => {
            if let Some(cmd) = a.input.get("command").and_then(Value::as_str) {
                format!("{}: {cmd}", a.tool)
            } else if let Some(d) = a.detail.as_str() {
                format!("{}: {d}", a.tool)
            } else {
                a.tool.clone()
            }
        }
    }
}

/// The icon of an approval's kind, as the approvals list draws it.
fn kind_icon(kind: &str) -> &'static str {
    match kind {
        "review" => "git-pull-request",
        "tool" => "terminal",
        "deploy" => "rocket",
        "payment" => "credit-card",
        _ => "shield-check",
    }
}

/// An agent's label for the waiting list and the feed: its role and cid, "agent #<cid>" when unknown.
fn label_of(state: &State, cid: u64) -> String {
    match state.agents.iter().find(|x| x.cid == cid) {
        Some(a) => roles::agent_label(&a.role, a.cid),
        None => roles::agent_label("", cid),
    }
}

/// The Activity card: the transcript events of the plaza's agents (or the project's), newest first.
fn activity<'a>(state: &'a State) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let body: Element<'a, Msg> = if state.feed.is_empty() {
        container(ui::faint("Waiting for activity…"))
            .width(Length::Fill)
            .align_x(Horizontal::Center)
            .into()
    } else {
        let now = now_ms();
        let mut list = column![].spacing(8);
        for item in &state.feed {
            let who = label_of(state, item.cid);
            list = list.push(
                row![
                    container(
                        text(ago(now, item.at_ms))
                            .font(theme::SANS)
                            .size(TYPE_META)
                            .color(p.ink_3)
                            .wrapping(Wrapping::None)
                    )
                    .width(Length::Fixed(72.0)),
                    ui::text_link(&who, Msg::Go(Id::Agent(item.cid))),
                    text(item.line.clone())
                        .font(theme::SANS)
                        .size(TYPE_UI)
                        .color(p.ink_2)
                        .width(Length::Fill),
                ]
                .spacing(12)
                .align_y(Vertical::Top),
            );
        }
        // The feed scrolls past 360px and is shorter than that when it has less to show.
        container(ui::scroll(list)).max_height(360.0).into()
    };
    ov_card(vec![head_title("Activity")], body, false)
}

/// The Projects card: a line per project still in progress, its monogram, title, stage and task count; it opens the
/// project's Work view.
fn projects<'a>(state: &'a State) -> Element<'a, Msg> {
    let Some((list, tasks)) = &state.projects else {
        return ov_card(
            vec![head_title("Projects")],
            ui::empty(
                "alert",
                "Projects are unavailable",
                "apid did not answer; they come back with it.",
            ),
            false,
        );
    };
    let active: Vec<&model::Project> = list.iter().filter(|p| p.live()).collect();
    if active.is_empty() {
        return ov_card(
            vec![head_title("Projects")],
            ui::empty(
                "projects",
                "No projects in progress",
                "Each goal you give the orchestrator becomes a project; its flow shows here.",
            ),
            false,
        );
    }
    let step_of = |cid: u64| match state.steps.get(&cid) {
        Some(step) => step.clone(),
        None => state
            .agents
            .iter()
            .find(|a| a.cid == cid)
            .and_then(|a| a.step.clone()),
    };
    let p = crate::theme::palette();
    let rows = active.into_iter().map(|pj| {
        let own = model::tasks_of(pj, tasks);
        let done = own.iter().filter(|t| t.state == "done").count();
        let yours = if model::plan_waits_for_owner(pj) {
            "waits for your approval"
        } else if model::waits_for_owner(Some(pj), &own, &step_of) {
            "waits for you"
        } else {
            ""
        };
        let counted = if own.is_empty() {
            String::new()
        } else {
            format!("{done} of {} tasks done", own.len())
        };
        let line = [model::stage_label(&pj.stage), counted, yours.to_string()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let body = row![
            crate::console::shell::pmono::<Msg>(&pj.id, &pj.title, 28.0),
            column![
                container(
                    text(pj.title.clone())
                        .font(SANS_MEDIUM)
                        .size(TYPE_UI)
                        .color(p.ink)
                        .wrapping(Wrapping::None)
                )
                .clip(true)
                .width(Length::Fill),
                container(ui::faint(line).wrapping(Wrapping::None))
                    .clip(true)
                    .width(Length::Fill),
            ]
            .spacing(4)
            .width(Length::Fill),
        ]
        .spacing(12)
        .align_y(Vertical::Center);
        let entry: Element<'a, Msg> = button(body)
            .on_press(Msg::Go(Id::WorkOn(pj.id.clone())))
            .padding([12.0, 16.0])
            .width(Length::Fill)
            .style(row_hover)
            .into();
        column![rule(p.rule), entry].into()
    });
    ov_card(vec![head_title("Projects")], column(rows).into(), true)
}

/// The Fleet card: a tile per agent (its dot, its label, its work or state), opening its transcript.
fn fleet<'a>(agents: Vec<&'a Agent>) -> Element<'a, Msg> {
    if agents.is_empty() {
        return ov_card(
            vec![head_title("Fleet")],
            ui::empty(
                "agents",
                "No agents yet",
                "Agents appear here as their VMs connect.",
            ),
            false,
        );
    }
    let mut grid = column![].spacing(8);
    for chunk in agents.chunks(FLEET_COLS) {
        let mut line = row![].spacing(8);
        for a in chunk {
            line = line.push(tile(a));
        }
        for _ in chunk.len()..FLEET_COLS {
            line = line.push(space::horizontal());
        }
        grid = grid.push(line);
    }
    ov_card(
        vec![head_title("Fleet")],
        container(grid).padding(16).width(Length::Fill).into(),
        true,
    )
}

fn tile<'a>(a: &'a Agent) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let tone = match a.status.as_str() {
        "working" => p.info,
        "idle" => p.ok,
        _ => p.ink_3,
    };
    let sub = match &a.work {
        Some(w) => clip(&w.prompt, 60),
        None if a.status == "offline" => "offline".to_string(),
        None if a.status == "asleep" => "asleep".to_string(),
        None if a.status == "dozing" => "asleep (light)".to_string(),
        None => a.role_label.clone(),
    };
    let body = row![
        ui::dot::<Msg>(tone),
        column![
            container(
                text(roles::agent_label(&a.role, a.cid))
                    .font(SANS_MEDIUM)
                    .size(TYPE_UI)
                    .color(p.ink)
                    .wrapping(Wrapping::None)
            )
            .clip(true)
            .width(Length::Fill),
            container(ui::faint(sub).wrapping(Wrapping::None))
                .clip(true)
                .width(Length::Fill),
        ]
        .spacing(4)
        .width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Vertical::Center);
    button(body)
        .on_press(Msg::Go(Id::Agent(a.cid)))
        .style(tile_style)
        .padding(12.0)
        .width(Length::FillPortion(1))
        .into()
}

/// A fleet tile: a ruled box on the sheet's second ground, lifted on hover.
fn tile_style(_: &Theme, status: button::Status) -> button::Style {
    let p = crate::theme::palette();
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(Background::Color(if hovered { p.hover } else { p.sheet_2 })),
        text_color: p.ink,
        border: Border {
            color: if hovered { p.ink_3 } else { p.rule },
            width: 1.0,
            radius: crate::theme::skin().radius.into(),
        },
        ..Default::default()
    }
}

/// A row's hover: the hover ground under the pointer, ink on it.
fn row_hover(_: &Theme, status: button::Status) -> button::Style {
    let p = crate::theme::palette();
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: hovered.then_some(Background::Color(p.hover)),
        text_color: p.ink,
        border: Border::default(),
        ..Default::default()
    }
}

/// The web's `.card.ov-card`: a raised card, its head (sans semibold, 48 tall) over a rule, its body padded 16 or
/// flush when the body brings its own rows.
fn ov_card<'a>(
    head: Vec<Element<'a, Msg>>,
    body: Element<'a, Msg>,
    flush: bool,
) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let head = container(
        row(head)
            .spacing(8)
            .align_y(Vertical::Center)
            .width(Length::Fill),
    )
    .padding(Padding {
        top: 8.0,
        right: 16.0,
        bottom: 8.0,
        left: 16.0,
    })
    .width(Length::Fill)
    .center_y(Length::Fixed(48.0));
    let body = container(body)
        .padding(if flush { 0.0 } else { 16.0 })
        .width(Length::Fill);
    container(column![head, rule(p.rule), body])
        .width(Length::Fill)
        .clip(true)
        .style(ui::card_style)
        .into()
}

/// A card's head title: sans semibold, ink (the web's `.ov-h`).
fn head_title<'a>(title: &str) -> Element<'a, Msg> {
    text(title.to_string())
        .font(SANS_SEMIBOLD)
        .size(TYPE_UI)
        .color(crate::theme::palette().ink)
        .wrapping(Wrapping::None)
        .into()
}

/// A card-shaped box holding one element (an empty state with no head): the raised ground, the rule.
fn boxed_card<'a>(content: Element<'a, Msg>) -> Element<'a, Msg> {
    container(content)
        .width(Length::Fill)
        .clip(true)
        .style(ui::card_style)
        .into()
}

/// A raised box with a rule, the web's `.stat` and `.home-ways` ground.
fn boxed(_: &Theme) -> container::Style {
    container::Style {
        background: Some(crate::theme::palette().raised.into()),
        border: Border {
            color: crate::theme::palette().rule,
            width: 1.0,
            radius: crate::theme::skin().radius_lg.into(),
        },
        ..Default::default()
    }
}

/// A full-width ruled line.
fn rule<'a>(color: Color) -> Element<'a, Msg> {
    container(Space::new().height(Length::Fixed(1.0)))
        .width(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(color)),
            ..Default::default()
        })
        .into()
}

/// The feed line for a transcript entry, if the feed shows that kind.
fn describe(e: &Entry) -> Option<String> {
    let d = &e.data;
    let text = |key: &str| d.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    match e.kind.as_str() {
        "work_start" => Some(format!(
            "started {}: {}",
            text("kind"),
            clip(&text("prompt"), 80)
        )),
        "work_end" => Some(format!("finished ({})", text("status"))),
        "tool_request" => Some(format!("called {}", text("name"))),
        "error" => Some(format!("error: {}", clip(&text("message"), 80))),
        _ => None,
    }
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

fn ago(now: u64, at: u64) -> String {
    let s = now.saturating_sub(at) / 1000;
    match s {
        0..=4 => "just now".to_string(),
        5..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
