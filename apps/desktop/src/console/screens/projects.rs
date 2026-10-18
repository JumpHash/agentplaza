//! The Projects screen (parity with the web console's `projects` view).
//!
//! The list of projects, with New project, and one project's page: its brief, the eight-stage
//! track, Pause, Resume and Archive, and three tabs: Plan (documents, critiques, tasks, repository
//! and history; the owner approves the plan or sends the project back to an earlier stage with a
//! reason), Team ([`super::project_team`]) and Settings ([`super::project_settings`]).

use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{Space, Stack, button, column, container, row, text, text_input};
use iced::{Alignment, Border, Color, Element, Font, Length, Subscription, Task, Theme, font};
use serde::Deserialize;
use serde_json::{Value, json};

use super::wizard::WizardMsg;
use super::{Id, ide, project_settings, project_team};
use crate::console::{Ctx, client, events, ui};

/// The document types a system viewer shows as plain content; any other file is not opened.
const OPENABLE: [&str; 9] = [
    "pdf", "md", "txt", "png", "jpg", "jpeg", "gif", "webp", "json",
];
use crate::theme;

/// The eight stages, in order (`PROJECT_STAGES` on the web).
const STAGES: [&str; 8] = [
    "brief",
    "research",
    "proposal",
    "critique",
    "approval",
    "implementation",
    "review",
    "delivered",
];
/// The stages in which the owner can approve the plan.
const APPROVABLE: [&str; 2] = ["critique", "approval"];
/// Serif semibold: titles, document names, the current stage (the web's `font-weight: 600`).
const SEMIBOLD: Font = Font {
    weight: font::Weight::Semibold,
    ..theme::SANS
};
/// Serif bold: the actor of a history entry (the web's `<strong>`).
const BOLD: Font = Font {
    weight: font::Weight::Bold,
    ..theme::SANS
};

/// One project, as the hub serves it (`GET /v1/projects/{id}`).
#[derive(Debug, Clone, Deserialize)]
pub struct Project {
    pub id: String,
    pub title: String,
    pub goal: String,
    pub kind: String,
    pub stage: String,
    pub revision: u64,
    pub repo: Option<String>,
    pub docs: Vec<Doc>,
    pub critiques: Vec<Critique>,
    pub tasks: Vec<String>,
    pub history: Vec<Event>,
    pub approved_by: Option<String>,
    pub updated_ms: i64,
    #[serde(default)]
    pub created_ms: i64,
    /// Paused: its agents take no work until it is resumed.
    #[serde(default)]
    pub paused: bool,
    /// Archived: nothing more is done for it.
    #[serde(default)]
    pub archived: bool,
}

impl Project {
    /// Whether it still has work: not archived, not delivered.
    pub fn live(&self) -> bool {
        !self.archived && self.stage != "delivered"
    }
}

/// A document a stage produced.
#[derive(Debug, Clone, Deserialize)]
pub struct Doc {
    pub kind: String,
    pub title: String,
    /// The hub file id behind the document, when it has one.
    #[serde(default)]
    pub file: Option<String>,
    pub repo_path: Option<String>,
    pub commit: Option<String>,
    pub summary: String,
    pub by: String,
    pub at_ms: i64,
}

/// A critique: approve, revise or comment.
#[derive(Debug, Clone, Deserialize)]
pub struct Critique {
    pub verdict: String,
    pub by: String,
    pub revision: u64,
    pub text: String,
    pub at_ms: i64,
}

/// One entry of a project's history.
#[derive(Debug, Clone, Deserialize)]
pub struct Event {
    pub at_ms: i64,
    pub by: String,
    pub what: String,
    pub note: String,
}

/// A project page's tabs: its plan and history, the agents working on it, its settings and budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Plan,
    Team,
    Settings,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Plan, Tab::Team, Tab::Settings];

    /// A tab by its name (`plan`, `team`, `settings`), as the web's route names it.
    pub fn named(name: &str) -> Option<Tab> {
        match name.trim() {
            "plan" => Some(Tab::Plan),
            "team" => Some(Tab::Team),
            "settings" => Some(Tab::Settings),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Plan => "Plan",
            Tab::Team => "Team",
            Tab::Settings => "Settings",
        }
    }
}

/// A send-back in progress: the stage it returns to and the reason the team must act on.
#[derive(Debug, Clone)]
pub struct SendBack {
    pub stage: String,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct State {
    pub error: Option<String>,
    /// The project list; None until the first load.
    pub list: Option<Vec<Project>>,
    /// The project open on its page, if any.
    pub open: Option<String>,
    /// The open project, as last read.
    pub detail: Option<Project>,
    /// A line confirming the last action ("Plan approved", "Sent back").
    pub notice: Option<&'static str>,
    /// The approval form's note, while it is open.
    pub approve: Option<String>,
    /// The send-back form, while it is open.
    pub send_back: Option<SendBack>,
    /// A reload from a hub event is in flight.
    pub loading: bool,
    /// A hub event came while a reload was in flight: one more reload when it lands.
    pub again: bool,
    /// The open project's tab.
    pub tab: Tab,
    /// The Archive confirmation is open.
    pub confirm_archive: bool,
    pub team: project_team::State,
    pub settings: project_settings::State,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    List(Result<Vec<Project>, String>),
    Detail(String, Result<Project, String>),
    /// An approval or send-back finished: the updated project, and the line to show.
    Acted(Result<Project, String>, &'static str),
    Open(String),
    Back,
    OpenApprove,
    ApproveNote(String),
    OpenSendBack,
    PickStage(String),
    Reason(String),
    SubmitApprove,
    SubmitSendBack,
    CancelForm,
    /// A hub event: the screen reloads when a project or task changed.
    Event(events::Event),
    /// Open a document's file in the system viewer, by its hub file id.
    OpenFile(String),
    Opened(Result<(), String>),
    /// Opens the project's repository in the IDE on its default branch (the branch is read first).
    OpenRepo(String),
    /// The repository's default branch, read for `OpenRepo`; the IDE opens on it when it is in.
    RepoBranch(String, Result<Value, String>),
    /// New project: the shell's dialog (the console opens it).
    NewProject,
    /// The open project's tab.
    Tab(Tab),
    Pause,
    Resume,
    /// Archive: asked (OpenArchive), then done or not.
    OpenArchive,
    Archive,
    CancelArchive,
    Team(project_team::Msg),
    Settings(project_settings::Msg),
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        Msg::Team(m) => project_team::go(m),
        Msg::Settings(m) => project_settings::go(m),
        _ => None,
    }
}

/// The screen as it opens on one project's page (the Work screen's plan cards open it this way); the load that
/// follows reads that project.
pub fn opened(id: String) -> State {
    opened_on(id, Tab::Plan)
}

/// The screen as it opens on one project's page, on `tab`.
pub fn opened_on(id: String, tab: Tab) -> State {
    State {
        open: Some(id),
        tab,
        ..Default::default()
    }
}

/// Loads the screen's data (called when it opens and on every poll tick): the list, or the open project and its tab.
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    match state.open.clone() {
        Some(id) => Task::batch([fetch_detail(ctx, id.clone()), load_tab(state, &id, ctx)]),
        None => fetch_list(ctx),
    }
}

/// The open tab's own data: the team, or the settings (the plan is the project itself).
fn load_tab(state: &State, id: &str, ctx: &Ctx) -> Task<Msg> {
    let live = state.detail.as_ref().is_none_or(Project::live);
    match state.tab {
        Tab::Plan => Task::none(),
        Tab::Team => project_team::load(id, live, ctx).map(Msg::Team),
        Tab::Settings => project_settings::load(id, ctx).map(Msg::Settings),
    }
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) | Msg::NewProject => Task::none(),
        Msg::Tab(tab) => {
            state.tab = tab;
            match state.open.clone() {
                Some(id) => load_tab(state, &id, ctx),
                None => Task::none(),
            }
        }
        Msg::Pause | Msg::Resume | Msg::Archive => {
            let Some(p) = state.detail.as_ref() else {
                return Task::none();
            };
            state.confirm_archive = false;
            let (path, done) = match msg {
                Msg::Pause => (
                    format!("/v1/projects/{}/pause", p.id),
                    "Paused: its agents wait",
                ),
                Msg::Resume => (format!("/v1/projects/{}/resume", p.id), "Resumed"),
                _ => (format!("/v1/projects/{}/archive", p.id), "Archived"),
            };
            let client = ctx.client.clone();
            client::task(
                move || client.post::<Project>(&path, &json!({})),
                move |r| Msg::Acted(r, done),
            )
        }
        Msg::OpenArchive => {
            state.confirm_archive = true;
            Task::none()
        }
        Msg::CancelArchive => {
            state.confirm_archive = false;
            Task::none()
        }
        Msg::Team(m) => {
            let Some(p) = state.detail.as_ref() else {
                return Task::none();
            };
            let (id, title, live) = (p.id.clone(), p.title.clone(), p.live());
            project_team::update(&mut state.team, m, &id, &title, live, ctx).map(Msg::Team)
        }
        Msg::Settings(m) => {
            let Some(id) = state.open.clone() else {
                return Task::none();
            };
            project_settings::update(&mut state.settings, m, &id, ctx).map(Msg::Settings)
        }
        Msg::OpenRepo(repo) => {
            let c = ctx.client.clone();
            let path = format!("/v1/git/repos/{repo}");
            client::task(
                move || c.get::<Value>(&path),
                move |res| Msg::RepoBranch(repo.clone(), res),
            )
        }
        Msg::RepoBranch(repo, res) => match res {
            Ok(info) => match info
                .get("default_branch")
                .and_then(Value::as_str)
                .filter(|b| !b.is_empty())
            {
                // The screen navigates through `Msg::Go`, the same as its other links.
                Some(branch) => Task::done(Msg::Go(Id::Ide(Some(ide::target(
                    &format!("repo:{repo}@{branch}"),
                    false,
                    None,
                ))))),
                None => {
                    state.error = Some(format!("{repo} has no default branch to open."));
                    Task::none()
                }
            },
            Err(e) => {
                state.error = Some(format!("Could not open {repo} in the IDE: {e}"));
                Task::none()
            }
        },
        Msg::List(res) => {
            match res {
                Ok(list) => {
                    state.list = Some(list);
                    state.error = None;
                }
                Err(e) => state.error = Some(e),
            }
            landed(state, ctx)
        }
        Msg::Detail(id, res) => {
            if state.open.as_deref() == Some(id.as_str()) {
                match res {
                    Ok(p) => {
                        state.detail = Some(p);
                        state.error = None;
                    }
                    Err(e) => state.error = Some(e),
                }
            }
            landed(state, ctx)
        }
        Msg::Event(event) => {
            if !matches!(event.kind.as_str(), "project" | "task") {
                return Task::none();
            }
            if state.loading {
                state.again = true;
                return Task::none();
            }
            state.loading = true;
            load(state, ctx)
        }
        Msg::OpenFile(id) => {
            let client = ctx.client.clone();
            client::task(move || open_file(&client, &id), Msg::Opened)
        }
        Msg::Opened(res) => {
            if let Err(e) = res {
                state.error = Some(e);
            }
            Task::none()
        }
        Msg::Acted(res, done) => {
            match res {
                Ok(p) => {
                    if state.open.as_deref() == Some(p.id.as_str()) {
                        state.detail = Some(p);
                        state.notice = Some(done);
                    }
                    state.approve = None;
                    state.send_back = None;
                    state.error = None;
                }
                Err(e) => state.error = Some(e),
            }
            // A pause, a resume or an archive moves agents: the open tab reads them again.
            match state.open.clone() {
                Some(id) => load_tab(state, &id, ctx),
                None => Task::none(),
            }
        }
        Msg::Open(id) => {
            *state = State {
                list: state.list.take(),
                ..opened(id.clone())
            };
            fetch_detail(ctx, id)
        }
        Msg::Back => {
            *state = State {
                list: state.list.take(),
                ..State::default()
            };
            fetch_list(ctx)
        }
        Msg::OpenApprove => {
            state.send_back = None;
            state.approve = Some(String::new());
            Task::none()
        }
        Msg::ApproveNote(note) => {
            if let Some(n) = state.approve.as_mut() {
                *n = note;
            }
            Task::none()
        }
        Msg::OpenSendBack => {
            let Some(p) = state.detail.as_ref() else {
                return Task::none();
            };
            let idx = stage_index(&p.stage);
            if idx == 0 {
                return Task::none();
            }
            state.approve = None;
            state.send_back = Some(SendBack {
                stage: STAGES[idx - 1].to_string(),
                reason: String::new(),
            });
            Task::none()
        }
        Msg::PickStage(stage) => {
            if let Some(sb) = state.send_back.as_mut() {
                sb.stage = stage;
            }
            Task::none()
        }
        Msg::Reason(reason) => {
            if let Some(sb) = state.send_back.as_mut() {
                sb.reason = reason;
            }
            Task::none()
        }
        Msg::CancelForm => {
            state.approve = None;
            state.send_back = None;
            Task::none()
        }
        Msg::SubmitApprove => {
            let Some(p) = state.detail.as_ref() else {
                return Task::none();
            };
            let Some(note) = state.approve.as_ref() else {
                return Task::none();
            };
            let client = ctx.client.clone();
            let path = format!("/v1/projects/{}/approve", p.id);
            let body = json!({ "note": note.trim() });
            client::task(
                move || client.post::<Project>(&path, &body),
                |r| Msg::Acted(r, "Plan approved"),
            )
        }
        Msg::SubmitSendBack => {
            let Some(p) = state.detail.as_ref() else {
                return Task::none();
            };
            let Some(sb) = state.send_back.as_ref() else {
                return Task::none();
            };
            let why = sb.reason.trim().to_string();
            if why.is_empty() {
                state.error = Some("Give a reason so the team knows what to change".into());
                return Task::none();
            }
            let client = ctx.client.clone();
            let path = format!("/v1/projects/{}/veto", p.id);
            let body = json!({ "stage": sb.stage, "reason": why });
            client::task(
                move || client.post::<Project>(&path, &body),
                |r| Msg::Acted(r, "Sent back"),
            )
        }
    }
}

pub fn view<'a>(state: &'a State, ctx: &'a Ctx) -> Element<'a, Msg> {
    match (&state.open, &state.detail) {
        (None, _) => list_view(state),
        (Some(_), Some(p)) => {
            let page = detail(state, p);
            // The Team tab's wizard spawns new agents for the project, over the page.
            if state.tab == Tab::Team && state.team.wizard.open {
                Stack::new()
                    .push(page)
                    .push(
                        super::wizard::view(&state.team.wizard, ctx)
                            .map(|m: WizardMsg| Msg::Team(project_team::Msg::Wizard(m))),
                    )
                    .into()
            } else {
                page
            }
        }
        (Some(_), None) => {
            // A failed read is "not found" on the web (the toast carries the reason).
            let body: Element<'a, Msg> = match &state.error {
                Some(_) => ui::empty("projects", "Project not found", "It may have been removed."),
                None => ui::loading(),
            };
            ui::sheet(
                ui::view_head("Project", vec![]),
                column![link("← All projects", Msg::Back), body]
                    .spacing(18)
                    .into(),
            )
        }
    }
}

fn fetch_list(ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(
        move || client.get::<Vec<Project>>("/v1/projects"),
        Msg::List,
    )
}

fn fetch_detail(ctx: &Ctx, id: String) -> Task<Msg> {
    let client = ctx.client.clone();
    let path = format!("/v1/projects/{id}");
    client::task(
        move || client.get::<Project>(&path),
        move |r| Msg::Detail(id.clone(), r),
    )
}

// ---- helpers -------------------------------------------------------------------------

fn stage_index(stage: &str) -> usize {
    STAGES.iter().position(|s| *s == stage).unwrap_or(0)
}

/// Sort rank of a document kind: its stage's place, unknown kinds last.
fn rank(kind: &str) -> usize {
    STAGES
        .iter()
        .position(|s| *s == kind)
        .unwrap_or(STAGES.len())
}

/// "brief" becomes "Brief".
fn label(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// A relative time, as the web's `ago`.
fn ago(ms: i64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let s = (now - ms).max(0) / 1000;
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

/// A serif semibold line in ink.
fn semibold<'a>(content: impl text::IntoFragment<'a>, size: f32) -> text::Text<'a> {
    text(content)
        .font(SEMIBOLD)
        .size(size)
        .color(crate::theme::palette().ink)
}

/// A text link in the signal colour, mono 12 (the web's `.pj-link`).
fn link<'a>(label_text: &str, msg: Msg) -> Element<'a, Msg> {
    button(ui::sans(
        label_text.to_string(),
        12.0,
        crate::theme::palette().signal,
    ))
    .padding(0)
    .style(|_: &Theme, _| button::Style {
        background: None,
        text_color: crate::theme::palette().signal,
        ..Default::default()
    })
    .on_press(msg)
    .into()
}

/// The eight stage boxes (10px squares, 1px rule edges): earlier stages filled in ink-3, the current
/// one in signal, later ones hollow; the stage name beside them.
fn track<'a>(stage: &str) -> Element<'a, Msg> {
    let idx = stage_index(stage);
    let segs = row(STAGES.iter().enumerate().map(|(i, _)| {
        let (fill, edge) = if i == idx {
            (
                crate::theme::palette().signal,
                crate::theme::palette().signal,
            )
        } else if i < idx {
            (crate::theme::palette().ink_3, crate::theme::palette().ink_3)
        } else {
            (Color::TRANSPARENT, crate::theme::palette().rule)
        };
        container(Space::new().width(8.0).height(8.0))
            .style(move |_: &Theme| container::Style {
                background: Some(fill.into()),
                border: Border {
                    color: edge,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .into()
    }))
    .spacing(3);
    row![
        segs,
        text(label(stage))
            .font(theme::SANS)
            .size(12.0)
            .color(crate::theme::palette().ink_2)
    ]
    .spacing(10)
    .align_y(Alignment::Center)
    .into()
}

/// The pipeline on the detail page: a ruled strip of eight cells. Done steps are ticked, the current
/// one is highlighted (sheet ground, signal underline, semibold), later ones faint.
fn pipeline<'a>(stage: &str) -> Element<'a, Msg> {
    let idx = stage_index(stage);
    let mut items: Vec<Element<'a, Msg>> = Vec::new();
    for (i, s) in STAGES.iter().enumerate() {
        if i > 0 {
            items.push(
                container(Space::new().width(1.0).height(Length::Fill))
                    .style(|_: &Theme| container::Style {
                        background: Some(crate::theme::palette().rule.into()),
                        ..Default::default()
                    })
                    .into(),
            );
        }
        let current = i == idx;
        let (mark, filled, fg) = if i < idx {
            (
                "✓".to_string(),
                Some(crate::theme::palette().ink),
                crate::theme::palette().ink,
            )
        } else if current {
            (
                (i + 1).to_string(),
                Some(crate::theme::palette().signal),
                crate::theme::palette().ink,
            )
        } else {
            ((i + 1).to_string(), None, crate::theme::palette().ink_3)
        };
        let edge = if i < idx {
            crate::theme::palette().ink
        } else if current {
            crate::theme::palette().signal
        } else {
            fg
        };
        let mark_box = container(ui::sans(
            mark,
            11.0,
            if filled.is_some() {
                crate::theme::palette().paper
            } else {
                fg
            },
        ))
        .width(20.0)
        .height(20.0)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |_: &Theme| container::Style {
            background: filled.map(Into::into),
            border: Border {
                color: edge,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        });
        let name = text(label(s))
            .font(if current { SEMIBOLD } else { theme::SANS })
            .size(12.0)
            .color(fg);
        let underline = if current {
            crate::theme::palette().signal
        } else {
            Color::TRANSPARENT
        };
        let cell = column![
            column![mark_box, name].spacing(6).padding([10, 8]),
            container(Space::new().height(3.0))
                .width(Length::Fill)
                .style(move |_: &Theme| container::Style {
                    background: Some(underline.into()),
                    ..Default::default()
                }),
        ]
        .width(Length::Fill);
        items.push(
            container(cell)
                .width(Length::Fill)
                .style(move |_: &Theme| container::Style {
                    background: if current {
                        Some(crate::theme::palette().sheet.into())
                    } else {
                        None
                    },
                    ..Default::default()
                })
                .into(),
        );
    }
    container(row(items))
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

// ---- the list ------------------------------------------------------------------------

/// A project's life as a badge: paused or archived (a delivered one says so in its stage).
fn life_badge<'a>(p: &Project) -> Option<Element<'a, Msg>> {
    if p.archived {
        Some(ui::badge("Archived", ui::Tone::Muted))
    } else if p.paused {
        Some(ui::badge("Paused", ui::Tone::Warn))
    } else {
        None
    }
}

fn row_item<'a>(p: &'a Project) -> Element<'a, Msg> {
    // A project that is over is listed after the live ones, its title faint.
    let title = if p.live() {
        semibold(p.title.as_str(), 15.0)
    } else {
        semibold(p.title.as_str(), 15.0).color(crate::theme::palette().ink_3)
    };
    let mut head = row![title, ui::badge(&p.kind, ui::Tone::Muted)]
        .spacing(10)
        .align_y(Alignment::Center);
    if let Some(b) = life_badge(p) {
        head = head.push(b);
    }
    let main = column![
        head,
        track(&p.stage),
        ui::sans(
            format!(
                "rev {} · {} docs · {} critiques · {} tasks",
                p.revision,
                p.docs.len(),
                p.critiques.len(),
                p.tasks.len()
            ),
            12.0,
            crate::theme::palette().ink_3
        ),
    ]
    .spacing(6)
    .width(Length::Fill);
    let side = column![ui::sans(
        format!("updated {}", ago(p.updated_ms)),
        12.0,
        crate::theme::palette().ink_3
    )]
    .align_x(Alignment::End);
    let entry = button(row![main, side].spacing(16).align_y(Alignment::Center))
        .on_press(Msg::Open(p.id.clone()))
        .padding([14, 4])
        .width(Length::Fill)
        .style(theme::register_row(false));
    // Each row is closed by a rule (the web's `.pj-row` border-bottom).
    column![
        entry,
        container(Space::new().height(1.0))
            .width(Length::Fill)
            .style(|_: &Theme| container::Style {
                background: Some(crate::theme::palette().rule.into()),
                ..Default::default()
            })
    ]
    .into()
}

fn list_view<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut body: Vec<Element<'a, Msg>> = vec![container(
        ui::prose("Agents work on one project at a time. Talk a new one through with the orchestrator, or open one yourself.").color(crate::theme::palette().ink_2),
    )
    .max_width(620)
    .into()];
    if let Some(e) = &state.error {
        body.push(ui::error_line(e));
    }
    match &state.list {
        None => body.push(ui::loading()),
        Some(list) if list.is_empty() => {
            body.push(ui::empty(
                "projects",
                "No projects yet.",
                "Start one in a conversation with the orchestrator: it drafts the tasks, the team and the budget, and you start it.",
            ));
            body.push(
                container(new_project_button())
                    .width(Length::Fill)
                    .align_x(Alignment::Center)
                    .into(),
            );
        }
        Some(list) => {
            // The web's `.pj-list` ruled register: a rule above the rows, one under each.
            body.push(
                container(Space::new().height(1.0))
                    .width(Length::Fill)
                    .style(|_: &Theme| container::Style {
                        background: Some(crate::theme::palette().rule.into()),
                        ..Default::default()
                    })
                    .into(),
            );
            // The live projects first, newest first; the ones that are over after them.
            let mut ordered: Vec<&Project> = list.iter().collect();
            ordered.sort_by_key(|p| (!p.live(), std::cmp::Reverse(p.created_ms)));
            body.push(column(ordered.into_iter().map(row_item)).spacing(0).into());
        }
    }
    ui::sheet(
        ui::view_head("Projects", vec![new_project_button()]),
        scrolled(body),
    )
}

/// A page's body, scrolling under its head: the shell leaves this screen's scrolling to it, so a long list or a
/// project's long tab scrolls instead of being squeezed into the window.
fn scrolled<'a>(body: Vec<Element<'a, Msg>>) -> Element<'a, Msg> {
    ui::scroll(column(body).spacing(18).padding(iced::Padding {
        top: 0.0,
        right: 14.0,
        bottom: 0.0,
        left: 0.0,
    }))
    .into()
}

fn new_project_button<'a>() -> Element<'a, Msg> {
    ui::btn(
        "New project",
        Some("plus"),
        ui::Kind::Primary,
        Some(Msg::NewProject),
    )
}

// ---- a project's page ----------------------------------------------------------------

fn detail<'a>(state: &'a State, p: &'a Project) -> Element<'a, Msg> {
    let idx = stage_index(&p.stage);

    let live = p.live();
    let mut badges: Vec<Element<'a, Msg>> = vec![
        ui::badge(&p.kind, ui::Tone::Muted),
        ui::badge(&label(&p.stage), ui::Tone::Signal),
    ];
    if let Some(b) = life_badge(p) {
        badges.push(b);
    }
    if let Some(by) = &p.approved_by {
        badges.push(ui::faint(format!("approved by {by}")).into());
    }

    let mut body: Vec<Element<'a, Msg>> = vec![
        link("← All projects", Msg::Back),
        ui::prose(p.goal.as_str()).into(),
        pipeline(&p.stage),
    ];

    let mut actions: Vec<Element<'a, Msg>> = Vec::new();
    if APPROVABLE.contains(&p.stage.as_str()) {
        actions.push(ui::btn(
            "Approve plan",
            None,
            ui::Kind::Primary,
            Some(Msg::OpenApprove),
        ));
    }
    if idx > 0 && live {
        actions.push(ui::btn(
            "Send back…",
            None,
            ui::Kind::Secondary,
            Some(Msg::OpenSendBack),
        ));
    }
    // The project as one flow: its plan, its tasks in the order they run, delivery.
    actions.push(ui::btn(
        "Open in Work",
        Some("commit"),
        ui::Kind::Secondary,
        Some(Msg::Go(Id::WorkOn(p.id.clone()))),
    ));
    if live && !p.paused {
        actions.push(ui::btn(
            "Pause",
            None,
            ui::Kind::Secondary,
            Some(Msg::Pause),
        ));
    }
    if live && p.paused {
        actions.push(ui::btn(
            "Resume",
            None,
            ui::Kind::Primary,
            Some(Msg::Resume),
        ));
    }
    if live {
        actions.push(ui::btn(
            "Archive…",
            None,
            ui::Kind::Ghost,
            Some(Msg::OpenArchive),
        ));
    }
    body.push(row(actions).spacing(8).wrap().into());
    if state.confirm_archive {
        body.push(ui::card(
            "Archive project?",
            column![
                ui::prose(format!("Nothing more is done for {}: its open tasks are withdrawn, its agents go back to the bench, and the rest of its budget goes back to the plaza. Its history stays.", p.title)),
                row![
                    ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CancelArchive)),
                    ui::btn("Archive", None, ui::Kind::Danger, Some(Msg::Archive)),
                ]
                .spacing(8),
            ]
            .spacing(10)
            .into(),
        ));
    }

    if let Some(note) = &state.approve {
        let field = text_input("A note for the team (optional)", note)
            .on_input(Msg::ApproveNote)
            .on_submit(Msg::SubmitApprove)
            .style(theme::input);
        let form = column![
            ui::label("Note"),
            field,
            row![
                ui::btn(
                    "Approve plan",
                    None,
                    ui::Kind::Primary,
                    Some(Msg::SubmitApprove)
                ),
                ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CancelForm)),
            ]
            .spacing(8),
        ]
        .spacing(10);
        body.push(ui::card("Approve plan", form.into()));
    }

    if let Some(sb) = &state.send_back {
        let picks = row(STAGES[..idx].iter().map(|s| {
            let kind = if *s == sb.stage {
                ui::Kind::Primary
            } else {
                ui::Kind::Secondary
            };
            ui::btn(&label(s), None, kind, Some(Msg::PickStage(s.to_string())))
        }))
        .spacing(6);
        let reason = text_input("What must change, and why", &sb.reason)
            .on_input(Msg::Reason)
            .on_submit(Msg::SubmitSendBack)
            .style(theme::input);
        let form = column![
            ui::label("Back to"),
            ui::faint("The stage the project returns to"),
            picks,
            ui::label("Reason"),
            reason,
            ui::btn(
                "Send back",
                None,
                ui::Kind::Danger,
                Some(Msg::SubmitSendBack)
            ),
        ]
        .spacing(10);
        body.push(ui::card("Send back", form.into()));
    }

    if let Some(e) = &state.error {
        body.push(ui::error_line(e));
    }
    if let Some(n) = state.notice {
        body.push(ui::faint(n).into());
    }

    let tabs: Vec<(Tab, &str)> = Tab::ALL.iter().map(|t| (*t, t.label())).collect();
    body.push(ui::tabs(&tabs, state.tab, Msg::Tab));
    match state.tab {
        Tab::Team => {
            body.push(project_team::view(&state.team, &p.id, live).map(Msg::Team));
            return page(p, badges, body);
        }
        Tab::Settings => {
            body.push(project_settings::view(&state.settings, &p.id, live).map(Msg::Settings));
            return page(p, badges, body);
        }
        Tab::Plan => {}
    }

    let mut sections: Vec<Element<'a, Msg>> = vec![
        ui::card("Documents", docs_body(p)),
        ui::card("Critiques", critiques_body(p)),
        ui::card("Tasks", tasks_body(p)),
    ];
    if let Some(repo) = &p.repo {
        let repo_card: Element<'a, Msg> = column![
            link(repo, Msg::Go(Id::History)),
            ui::btn_sm(
                "Open repo",
                Some("square-code"),
                ui::Kind::Ghost,
                Some(Msg::OpenRepo(repo.clone()))
            ),
        ]
        .spacing(8)
        .into();
        sections.push(ui::card("Repository", repo_card));
    }
    sections.push(ui::card("History", history_body(p)));
    body.push(column(sections).spacing(16).into());
    page(p, badges, body)
}

/// A project's page: its title and badges over the body.
fn page<'a>(
    p: &'a Project,
    badges: Vec<Element<'a, Msg>>,
    body: Vec<Element<'a, Msg>>,
) -> Element<'a, Msg> {
    ui::sheet(ui::view_head(&p.title, badges), scrolled(body))
}

fn docs_body<'a>(p: &'a Project) -> Element<'a, Msg> {
    if p.docs.is_empty() {
        return ui::faint("No documents yet.").into();
    }
    // Group by kind, in the order the kinds first appear; then order the groups by stage.
    let mut groups: Vec<(&'a str, Vec<&'a Doc>)> = Vec::new();
    for d in &p.docs {
        match groups.iter().position(|(k, _)| *k == d.kind) {
            Some(i) => groups[i].1.push(d),
            None => groups.push((d.kind.as_str(), vec![d])),
        }
    }
    groups.sort_by_key(|(k, _)| rank(k));
    column(groups.into_iter().map(|(kind, docs)| -> Element<'a, Msg> {
        let items = column(docs.into_iter().map(doc_item)).spacing(16);
        column![ui::label(&label(kind)), items].spacing(10).into()
    }))
    .spacing(24)
    .into()
}

fn doc_item<'a>(d: &'a Doc) -> Element<'a, Msg> {
    let mut col = column![
        row![
            semibold(d.title.as_str(), 15.0),
            ui::badge(&d.kind, ui::Tone::Muted)
        ]
        .spacing(10)
        .align_y(Alignment::Center),
        ui::sans(
            format!("by {} · {}", d.by, ago(d.at_ms)),
            12.0,
            crate::theme::palette().ink_3
        ),
    ]
    .spacing(6);
    if !d.summary.is_empty() {
        col = col.push(ui::markdown(&d.summary));
    }
    if let Some(id) = &d.file {
        col = col.push(link("Open", Msg::OpenFile(id.clone())));
    }
    if let Some(path) = &d.repo_path {
        let commit = d
            .commit
            .as_deref()
            .map(|c| format!(" @ {}", c.get(..7).unwrap_or(c)))
            .unwrap_or_default();
        col = col.push(ui::mono(
            format!("{path}{commit}"),
            12.0,
            crate::theme::palette().ink_2,
        ));
    }
    col.into()
}

fn critiques_body<'a>(p: &'a Project) -> Element<'a, Msg> {
    if p.critiques.is_empty() {
        return ui::faint("No critiques yet.").into();
    }
    column(p.critiques.iter().map(critique_item))
        .spacing(16)
        .into()
}

fn critique_item<'a>(c: &'a Critique) -> Element<'a, Msg> {
    let tone = match c.verdict.as_str() {
        "approve" => ui::Tone::Ok,
        "revise" => ui::Tone::Danger,
        _ => ui::Tone::Muted,
    };
    let head = row![
        ui::badge(&c.verdict, tone),
        ui::prose(format!("by {}", c.by)),
        ui::sans(
            format!("revision {}", c.revision),
            12.0,
            crate::theme::palette().ink_3
        ),
        ui::sans(ago(c.at_ms), 12.0, crate::theme::palette().ink_3),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let mut col = column![head].spacing(6);
    if !c.text.is_empty() {
        col = col.push(ui::markdown(&c.text));
    }
    col.into()
}

fn tasks_body<'a>(p: &'a Project) -> Element<'a, Msg> {
    if p.tasks.is_empty() {
        return ui::faint("No tasks yet.").into();
    }
    column![
        column(
            p.tasks
                .iter()
                .map(|t| link(t, Msg::Go(Id::Task(t.clone())))),
        )
        .spacing(8),
        link(
            "See them in the order they run, in Work",
            Msg::Go(Id::WorkOn(p.id.clone()))
        ),
    ]
    .spacing(12)
    .into()
}

fn history_body<'a>(p: &'a Project) -> Element<'a, Msg> {
    if p.history.is_empty() {
        return ui::faint("Nothing recorded yet.").into();
    }
    let mut events: Vec<&'a Event> = p.history.iter().collect();
    events.sort_by_key(|e| e.at_ms);
    column(events.into_iter().map(event_row)).spacing(8).into()
}

fn event_row<'a>(e: &'a Event) -> Element<'a, Msg> {
    let mut line = row![
        ui::sans(ago(e.at_ms), 12.0, crate::theme::palette().ink_3),
        text(e.by.as_str())
            .font(BOLD)
            .size(13.0)
            .color(crate::theme::palette().ink),
        ui::prose(e.what.as_str()).size(13.0),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    if !e.note.is_empty() {
        line = line.push(ui::sans(
            format!("— {}", e.note),
            12.0,
            crate::theme::palette().ink_3,
        ));
    }
    line.into()
}

/// The hub events this screen follows live: a project or task changed, so the page reloads.
pub fn subscription(ctx: &Ctx) -> Subscription<Msg> {
    events::subscription(&ctx.client).map(Msg::Event)
}

/// A load landed: the reload that an event asked for meanwhile runs now.
fn landed(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    state.loading = false;
    if std::mem::take(&mut state.again) {
        state.loading = true;
        load(state, ctx)
    } else {
        Task::none()
    }
}

/// The part of a file's record this screen needs: its name, which gives the extension and the temp name.
#[derive(Debug, Deserialize)]
struct FileMeta {
    name: String,
}

/// Saves one document's file under the temp directory and opens it in the system's viewer, as the
/// web opens the file in a new tab. The file's record (`GET /v1/files/{id}`) names it; a file of
/// another type, or one whose name has no such extension, is refused.
fn open_file(client: &client::Client, id: &str) -> Result<(), String> {
    let meta: FileMeta = client.get(&format!("/v1/files/{id}"))?;
    let file = std::path::Path::new(&meta.name)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| "the document has no file name".to_string())?;
    let ext = std::path::Path::new(file)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !OPENABLE.contains(&ext.as_str()) {
        return Err(format!("this file cannot be opened here: {file}"));
    }
    let path = format!("/v1/files/{id}/content");
    let reply = apz_http::client::request(&client.target, "GET", &path, None, client.timeout)
        .map_err(|e| e.to_string())?;
    if !(200..300).contains(&reply.status) {
        return Err(format!("HTTP {}", reply.status));
    }
    let dir = std::env::temp_dir().join("apz-documents").join(id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let target = dir.join(file);
    std::fs::write(&target, &reply.body).map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = std::process::Command::new("xdg-open");
    command
        .arg(&target)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}
