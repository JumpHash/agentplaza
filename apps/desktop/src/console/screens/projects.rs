//! The Projects screen (parity with the web console's `projects` view).
//!
//! The list of projects, with New project, and one project's page: its brief, the eight-stage
//! track, Pause, Resume and Archive, and its tabs: Plan (documents, critiques, tasks, repository
//! and history; the owner approves the plan or sends the project back to an earlier stage with a
//! reason), Maintenance, Environments and Settings ([`super::project_settings`]).

use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{Space, button, column, container, row, text, text_input};
use iced::{Alignment, Border, Color, Element, Font, Length, Subscription, Task, Theme, font};
use serde::Deserialize;
use serde_json::json;

use super::{Id, budget, ide, project_environments, project_maintenance, project_settings};
use crate::console::shell::pmono;
use crate::console::work_api::OwnerKind;
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
    /// How it is worked: `build`, or `maintenance` (its agents watch it and propose; the owner picks).
    #[serde(default)]
    pub mode: String,
}

impl Project {
    /// Whether it still has work: not archived, and not delivered unless it is in maintenance.
    pub fn live(&self) -> bool {
        !self.archived && (self.stage != "delivered" || self.in_maintenance())
    }

    pub fn in_maintenance(&self) -> bool {
        self.mode == "maintenance"
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

/// A project page's tabs: its plan and history, its maintenance, its environments, its settings and budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Plan,
    Maintenance,
    Environments,
    /// The project's budget: its cap, what is spent and left, its ledger, and its tasks' budgets.
    Budget,
    Settings,
}

impl Tab {
    /// A tab by its name (`plan`, `maintenance`, `environments`, `settings`), as the web's route names it.
    pub fn named(name: &str) -> Option<Tab> {
        match name.trim() {
            "plan" => Some(Tab::Plan),
            "maintenance" => Some(Tab::Maintenance),
            "environments" => Some(Tab::Environments),
            "budget" => Some(Tab::Budget),
            "settings" => Some(Tab::Settings),
            _ => None,
        }
    }
}

/// A send-back in progress: the stage it returns to and the reason the agents must act on.
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
    /// The More menu (send back, archive) is open.
    pub more: bool,
    pub settings: project_settings::State,
    pub maintenance: project_maintenance::State,
    pub environments: project_environments::State,
    pub budget: budget::Panel,
    /// The mode chooser is open.
    pub choose_mode: bool,
    /// Files being added to the repository: the line saying how it went.
    pub files_note: Option<String>,
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
    /// Opens the project's repository in the IDE at `main`, which every Weft repository has.
    OpenRepo(String),
    /// New project: the shell's dialog (the console opens it).
    NewProject,
    /// The open project's tab.
    Tab(Tab),
    Pause,
    Resume,
    /// Archive: asked (OpenArchive), then done or not.
    /// Opens or shuts the More menu on the page.
    ToggleMore,
    OpenArchive,
    Archive,
    CancelArchive,
    Settings(project_settings::Msg),
    Maintenance(project_maintenance::Msg),
    Environments(project_environments::Msg),
    Budget(budget::Msg),
    /// The mode chooser: open it, pick a mode (`build` or `maintenance`), or close it.
    OpenMode,
    SetMode(&'static str),
    CancelMode,
    /// Add files to the repository: pick them (files or a folder), then send them; or they were dropped on the window.
    PickFiles(bool),
    FilesPicked(Vec<std::path::PathBuf>),
    FilesAdded(Result<serde_json::Value, String>),
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        Msg::Settings(m) => project_settings::go(m),
        Msg::Maintenance(m) => project_maintenance::go(m),
        Msg::Environments(m) => project_environments::go(m),
        Msg::Budget(m) => budget::go(m),
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

/// The open tab's own data (the plan is the project itself, so it has none).
fn load_tab(state: &mut State, id: &str, ctx: &Ctx) -> Task<Msg> {
    match state.tab {
        Tab::Plan => Task::none(),
        Tab::Maintenance => project_maintenance::load(id, ctx).map(Msg::Maintenance),
        Tab::Environments => project_environments::load(id, ctx).map(Msg::Environments),
        Tab::Settings => project_settings::load(id, ctx).map(Msg::Settings),
        Tab::Budget => {
            if !state.budget.owned_by(OwnerKind::Project, id) {
                state.budget = budget::Panel::new(OwnerKind::Project, id);
            }
            budget::load(&mut state.budget, ctx).map(Msg::Budget)
        }
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
        Msg::ToggleMore => {
            state.more = !state.more;
            Task::none()
        }
        Msg::OpenArchive => {
            state.more = false;
            state.confirm_archive = true;
            Task::none()
        }
        Msg::CancelArchive => {
            state.confirm_archive = false;
            Task::none()
        }
        Msg::Settings(m) => {
            let Some(id) = state.open.clone() else {
                return Task::none();
            };
            project_settings::update(&mut state.settings, m, &id, ctx).map(Msg::Settings)
        }
        Msg::Maintenance(m) => {
            let Some(id) = state.open.clone() else {
                return Task::none();
            };
            project_maintenance::update(&mut state.maintenance, m, &id, ctx).map(Msg::Maintenance)
        }
        Msg::Environments(m) => {
            let Some(id) = state.open.clone() else {
                return Task::none();
            };
            project_environments::update(&mut state.environments, m, &id, ctx)
                .map(Msg::Environments)
        }
        Msg::Budget(m) => budget::update(&mut state.budget, m, ctx).map(Msg::Budget),
        Msg::OpenMode => {
            state.choose_mode = true;
            Task::none()
        }
        Msg::CancelMode => {
            state.choose_mode = false;
            Task::none()
        }
        Msg::SetMode(mode) => {
            state.choose_mode = false;
            let Some(id) = state.open.clone() else {
                return Task::none();
            };
            let client = ctx.client.clone();
            let done = if mode == "maintenance" {
                "In maintenance: its agents watch it and propose"
            } else {
                "In build: its agents make it"
            };
            client::task(
                move || {
                    client.post::<Project>(
                        &format!("/v1/projects/{id}/mode"),
                        &json!({ "mode": mode }),
                    )
                },
                move |r| Msg::Acted(r, done),
            )
        }
        Msg::PickFiles(folder) => Task::perform(
            async move {
                let dialog = rfd::AsyncFileDialog::new();
                let picked = if folder {
                    dialog.pick_folder().await.map(|f| vec![f])
                } else {
                    dialog.pick_files().await
                };
                picked
                    .unwrap_or_default()
                    .into_iter()
                    .map(|f| f.path().to_path_buf())
                    .collect()
            },
            Msg::FilesPicked,
        ),
        Msg::FilesPicked(paths) => {
            let Some(id) = state.open.clone() else {
                return Task::none();
            };
            if paths.is_empty() {
                return Task::none();
            }
            state.files_note = Some(format!("Sending {}…", files_text(paths.len())));
            let client = ctx.client.clone();
            client::task(
                move || crate::console::files::upload_to_project(&client, &id, &paths, None),
                Msg::FilesAdded,
            )
        }
        Msg::FilesAdded(res) => {
            state.files_note = Some(match res {
                Ok(v) => {
                    let n = v["files"].as_u64().unwrap_or(0);
                    let skipped = v["skipped"].as_u64().unwrap_or(0);
                    let left = if skipped > 0 {
                        format!(", {skipped} left out")
                    } else {
                        String::new()
                    };
                    format!(
                        "Added {} as change {}{left}.",
                        files_text(n as usize),
                        v["landed"]["change"].as_str().unwrap_or("?")
                    )
                }
                Err(e) => format!("Not sent: {e}"),
            });
            Task::none()
        }
        // The screen navigates through `Msg::Go`, the same as its other links.
        Msg::OpenRepo(repo) => Task::done(Msg::Go(Id::Ide(Some(ide::target(
            &format!("repo:{repo}@main"),
            false,
            None,
        ))))),
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
            // The budget panel follows its own frames (its budget, its sessions and its cards).
            let panel = budget::update(&mut state.budget, budget::Msg::Event(event.clone()), ctx)
                .map(Msg::Budget);
            if !matches!(
                event.kind.as_str(),
                "project" | "task" | "maintenance" | "environment"
            ) {
                return panel;
            }
            if state.loading {
                state.again = true;
                return panel;
            }
            state.loading = true;
            Task::batch([panel, load(state, ctx)])
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
            state.more = false;
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
                state.error = Some("Give a reason so the agents know what to change".into());
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

pub fn view<'a>(state: &'a State) -> Element<'a, Msg> {
    match (&state.open, &state.detail) {
        (None, _) => list_view(state),
        (Some(_), Some(p)) => detail(state, p),
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

/// A stage's bar colour: the done ones are ok at 70%, the current one is signal, the rest are rule-strong.
fn stage_fill(i: usize, idx: usize) -> Color {
    let p = crate::theme::palette();
    if i < idx {
        Color { a: 0.7, ..p.ok }
    } else if i == idx {
        p.signal
    } else {
        p.rule_strong
    }
}

/// The eight stage bars on a list row (the web's `.pj-track`): thin, rounded, the same width each.
fn track<'a>(stage: &str) -> Element<'a, Msg> {
    let idx = stage_index(stage);
    row(STAGES.iter().enumerate().map(|(i, _)| {
        container(Space::new().width(Length::Fill).height(4.0))
            .width(Length::Fill)
            .style(move |_: &Theme| container::Style {
                background: Some(stage_fill(i, idx).into()),
                border: Border::default().rounded(2.0),
                ..Default::default()
            })
            .into()
    }))
    .spacing(4)
    .width(Length::Fill)
    .into()
}

/// The pipeline on the detail page (the web's `.pj-pipe`): eight steps, each a bar over its name. The done ones are
/// ticked, the current one is ink and semibold, the later ones faint.
fn pipeline<'a>(stage: &str) -> Element<'a, Msg> {
    let idx = stage_index(stage);
    row(STAGES.iter().enumerate().map(|(i, s)| {
        let p = crate::theme::palette();
        let current = i == idx;
        let (ink, font) = if i < idx {
            (p.ink_2, theme::SANS)
        } else if current {
            (p.ink, SEMIBOLD)
        } else {
            (p.ink_3, theme::SANS)
        };
        let mut name = row![].spacing(4).align_y(Alignment::Center);
        if i < idx {
            name = name.push(ui::icon("check", 11.0, p.ok));
        }
        name = name.push(text(label(s)).font(font).size(12.0).color(ink));
        let bar = container(Space::new().width(Length::Fill).height(4.0))
            .width(Length::Fill)
            .style(move |_: &Theme| container::Style {
                background: Some(stage_fill(i, idx).into()),
                border: Border::default().rounded(2.0),
                ..Default::default()
            });
        column![bar, name].spacing(6).width(Length::Fill).into()
    }))
    .spacing(8)
    .width(Length::Fill)
    .into()
}

// ---- the list ------------------------------------------------------------------------

/// A project's life as a badge: paused or archived (a delivered one says so in its stage).
fn life_badge<'a>(p: &Project) -> Option<Element<'a, Msg>> {
    if p.archived {
        Some(ui::badge("Archived", ui::Tone::Muted))
    } else if p.paused {
        Some(ui::badge("Paused", ui::Tone::Warn))
    } else if p.in_maintenance() {
        Some(ui::badge("Maintenance", ui::Tone::Info))
    } else {
        None
    }
}

/// "1 file", "12 files".
fn files_text(n: usize) -> String {
    if n == 1 {
        "1 file".into()
    } else {
        format!("{n} files")
    }
}

/// One project on the list (the web's `.pj-row`): a bordered card, its title and badges over the stage bars, the
/// stage's name and the counts, and the update time at the right.
fn row_item<'a>(p: &'a Project) -> Element<'a, Msg> {
    let ink_2 = crate::theme::palette().ink_2;
    let ink_3 = crate::theme::palette().ink_3;
    // A project that is over is listed after the live ones, its title faint.
    let title = if p.live() {
        semibold(p.title.as_str(), 14.0)
    } else {
        semibold(p.title.as_str(), 14.0).color(ink_3)
    };
    let mut head = row![title, ui::badge(&p.kind, ui::Tone::Muted)]
        .spacing(8)
        .align_y(Alignment::Center);
    if let Some(b) = life_badge(p) {
        head = head.push(b);
    }
    let main = column![
        head,
        track(&p.stage),
        ui::sans(label(&p.stage), 13.0, ink_2),
        ui::sans(
            format!(
                "rev {} · {} docs · {} critiques · {} tasks",
                p.revision,
                p.docs.len(),
                p.critiques.len(),
                p.tasks.len()
            ),
            12.0,
            ink_3
        ),
    ]
    .spacing(8)
    .width(Length::Fill);
    let side = container(ui::sans(
        format!("updated {}", ago(p.updated_ms)),
        12.0,
        ink_3,
    ))
    .width(144.0)
    .align_x(Alignment::End);
    button(row![main, side].spacing(16).align_y(Alignment::Start))
        .on_press(Msg::Open(p.id.clone()))
        .padding(16)
        .width(Length::Fill)
        .style(move |_: &Theme, status: button::Status| {
            let p = crate::theme::palette();
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: Some(if hovered { p.hover } else { p.sheet }.into()),
                text_color: p.ink,
                border: Border {
                    color: p.rule,
                    width: 1.0,
                    radius: crate::theme::skin().radius_lg.into(),
                },
                ..Default::default()
            }
        })
        .into()
}

fn list_view<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut body: Vec<Element<'a, Msg>> = vec![ui::sans(
        "Agents work on one project at a time. Talk a new one through with the orchestrator, or open one yourself.",
        13.0,
        crate::theme::palette().ink_3,
    )
    .into()];
    if let Some(e) = &state.error {
        body.push(ui::error_line(e));
    }
    match &state.list {
        None => body.push(ui::loading()),
        Some(list) if list.is_empty() => {
            body.push(ui::empty(
                "commit",
                "No projects yet.",
                "Start one in a conversation with the orchestrator: it drafts the tasks and the budget, and you start it.",
            ));
            body.push(
                container(new_project_button())
                    .width(Length::Fill)
                    .align_x(Alignment::Center)
                    .into(),
            );
        }
        Some(list) => {
            // The live projects first, newest first; the ones that are over after them.
            let mut ordered: Vec<&Project> = list.iter().collect();
            ordered.sort_by_key(|p| (!p.live(), std::cmp::Reverse(p.created_ms)));
            body.push(column(ordered.into_iter().map(row_item)).spacing(8).into());
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
    let p_ink_2 = crate::theme::palette().ink_2;
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
        container(ui::prose(p.goal.as_str()).color(p_ink_2))
            .max_width(720.0)
            .into(),
        pipeline(&p.stage),
    ];

    // The page's actions (the web's `.pj-actions`): the common ones at the left, Approve and Resume at the right
    // end, and More (send back, archive) behind a menu.
    let mut out: Vec<Element<'a, Msg>> = vec![ui::btn_sm(
        "Open in Work",
        Some("commit"),
        ui::Kind::Secondary,
        Some(Msg::Go(Id::WorkOn(p.id.clone()))),
    )];
    if let Some(repo) = &p.repo {
        out.push(ui::btn_sm(
            "Open repo",
            None,
            ui::Kind::Secondary,
            Some(Msg::OpenRepo(repo.clone())),
        ));
    }
    if live && !p.paused {
        out.push(ui::btn_sm(
            "Pause",
            Some("pause"),
            ui::Kind::Secondary,
            Some(Msg::Pause),
        ));
    }
    if !p.archived {
        out.push(ui::btn_sm(
            if p.in_maintenance() {
                "Mode: Maintenance"
            } else {
                "Mode: Build"
            },
            Some("chevron-down"),
            ui::Kind::Secondary,
            Some(Msg::OpenMode),
        ));
    }
    let mut end: Vec<Element<'a, Msg>> = Vec::new();
    if live {
        end.push(ui::btn_sm(
            "More",
            Some("ellipsis"),
            ui::Kind::Secondary,
            Some(Msg::ToggleMore),
        ));
    }
    if APPROVABLE.contains(&p.stage.as_str()) {
        end.push(ui::btn_sm(
            "Approve plan",
            None,
            ui::Kind::Primary,
            Some(Msg::OpenApprove),
        ));
    }
    if live && p.paused {
        end.push(ui::btn_sm(
            "Resume",
            Some("play"),
            ui::Kind::Primary,
            Some(Msg::Resume),
        ));
    }
    body.push(
        row![
            container(row(out).spacing(8).wrap()).width(Length::Fill),
            row(end).spacing(8).align_y(Alignment::Center),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
    );
    if state.more && live {
        let mut items: Vec<Element<'a, Msg>> = Vec::new();
        if idx > 0 {
            items.push(ui::btn_sm(
                "Send back…",
                Some("undo-2"),
                ui::Kind::Secondary,
                Some(Msg::OpenSendBack),
            ));
        }
        items.push(ui::btn_sm(
            "Archive…",
            Some("archive"),
            ui::Kind::Secondary,
            Some(Msg::OpenArchive),
        ));
        body.push(ui::card("More", row(items).spacing(8).wrap().into()));
    }
    if state.confirm_archive {
        body.push(ui::card(
            "Archive project?",
            column![
                ui::prose(format!("Nothing more is done for {}: its open tasks are withdrawn, its agents stop working on it, and the rest of its budget goes back to the plaza. Its history stays.", p.title)),
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
    if state.choose_mode {
        let current = if p.in_maintenance() {
            "maintenance"
        } else {
            "build"
        };
        let choice =
            |mode: &'static str, name: &'static str, what: &'static str| -> Element<'a, Msg> {
                let now = if mode == current { " (now)" } else { "" };
                column![
                    ui::btn(
                        &format!("{name}{now}"),
                        None,
                        if mode == current {
                            ui::Kind::Ghost
                        } else {
                            ui::Kind::Primary
                        },
                        (mode != current).then_some(Msg::SetMode(mode)),
                    ),
                    ui::faint(what),
                ]
                .spacing(4)
                .into()
            };
        let mut form = column![
            choice("build", "Build", "The project is being made: agents plan the work, post tasks and build them."),
            choice(
                "maintenance",
                "Maintenance",
                "It is (nearly) finished: agents read its analytics, user reports and feedback, keep a map of it and propose work. Nothing runs until you pick a proposal."
            ),
        ]
        .spacing(12);
        if p.stage == "delivered" {
            form = form.push(ui::faint(
                "It is delivered: back in build it opens again at implementation.",
            ));
        }
        form = form.push(ui::btn(
            "Cancel",
            None,
            ui::Kind::Ghost,
            Some(Msg::CancelMode),
        ));
        body.push(ui::card("How this project is worked", form.into()));
    }

    if let Some(note) = &state.approve {
        let field = text_input("A note for the agents (optional)", note)
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

    body.push(tab_bar(p, state.tab));
    match state.tab {
        Tab::Settings => {
            body.push(project_settings::view(&state.settings, &p.id, live).map(Msg::Settings));
            return page(p, badges, body);
        }
        Tab::Maintenance => {
            body.push(
                project_maintenance::view(&state.maintenance, &p.id, p.in_maintenance())
                    .map(Msg::Maintenance),
            );
            return page(p, badges, body);
        }
        Tab::Environments => {
            body.push(
                project_environments::view(&state.environments, &p.id).map(Msg::Environments),
            );
            return page(p, badges, body);
        }
        Tab::Budget => {
            body.push(budget::view(&state.budget).map(Msg::Budget));
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
    if !p.archived && p.kind == "code" {
        let mut files = column![
            ui::faint("Choose files or a folder, or drop them on the window: they land on main as one change of yours. .git, node_modules and .DS_Store are left out."),
            row![
                ui::btn_sm("Choose files", Some("file"), ui::Kind::Secondary, Some(Msg::PickFiles(false))),
                ui::btn_sm("Choose a folder", Some("file"), ui::Kind::Secondary, Some(Msg::PickFiles(true))),
            ]
            .spacing(8),
        ]
        .spacing(8);
        if let Some(note) = &state.files_note {
            files = files.push(ui::faint(note.as_str()));
        }
        sections.push(ui::card("Add files", files.into()));
    }
    sections.push(ui::card("History", history_body(p)));
    body.push(column(sections).spacing(16).into());
    page(p, badges, body)
}

/// The page's tabs (the web's `.pj-tabs`): the ones drawn here, and the console's own Work, Tasks and Weft screens for the
/// project. Selected: ink with a signal rule under it; the rest faint.
fn tab_bar<'a>(p: &'a Project, tab: Tab) -> Element<'a, Msg> {
    let items: [(&str, Msg, bool); 8] = [
        ("Plan", Msg::Tab(Tab::Plan), tab == Tab::Plan),
        ("Work", Msg::Go(Id::WorkOn(p.id.clone())), false),
        ("Tasks", Msg::Go(Id::Tasks), false),
        ("Weft", Msg::Go(Id::History), false),
        (
            "Maintenance",
            Msg::Tab(Tab::Maintenance),
            tab == Tab::Maintenance,
        ),
        (
            "Environments",
            Msg::Tab(Tab::Environments),
            tab == Tab::Environments,
        ),
        ("Budget", Msg::Tab(Tab::Budget), tab == Tab::Budget),
        ("Settings", Msg::Tab(Tab::Settings), tab == Tab::Settings),
    ];
    let cells = items.into_iter().map(|(name, msg, on)| {
        let ink = if on {
            crate::theme::palette().ink
        } else {
            crate::theme::palette().ink_3
        };
        let cell = column![
            container(ui::sans(name, 13.0, ink))
                .height(34.0)
                .align_y(Alignment::Center),
            container(Space::new().width(Length::Fill).height(2.0))
                .width(Length::Fill)
                .style(move |_: &Theme| container::Style {
                    background: on.then(|| crate::theme::palette().action.into()),
                    ..Default::default()
                }),
        ];
        button(cell)
            .padding(0)
            .style(|_: &Theme, _| button::Style {
                background: None,
                ..Default::default()
            })
            .on_press(msg)
            .into()
    });
    column![row(cells).spacing(22), ui::hairline(),].into()
}

/// A project's page: the back link, its monogram, title and badges over the body (the web's `.pj-top`).
fn page<'a>(
    p: &'a Project,
    badges: Vec<Element<'a, Msg>>,
    body: Vec<Element<'a, Msg>>,
) -> Element<'a, Msg> {
    let back = button(ui::sans(
        "← All projects",
        12.0,
        crate::theme::palette().ink_3,
    ))
    .padding(0)
    .style(|_: &Theme, _| button::Style {
        background: None,
        ..Default::default()
    })
    .on_press(Msg::Back);
    let top = row![
        pmono(&p.id, &p.title, 40.0),
        column![
            semibold(p.title.as_str(), 23.0),
            row(badges).spacing(8).align_y(Alignment::Center).wrap(),
        ]
        .spacing(4)
        .width(Length::Fill),
    ]
    .spacing(12)
    .align_y(Alignment::Center);
    ui::sheet(column![back, top].spacing(4).into(), scrolled(body))
}

/// A row of a ruled list (the web's `.pj-docrow + .pj-docrow`): a rule above it, unless it is the first.
fn ruled<'a>(first: bool, content: Element<'a, Msg>) -> Element<'a, Msg> {
    if first {
        content
    } else {
        column![ui::hairline(), content].into()
    }
}

/// The documents in stage order (the web's `docsInOrder`), as ruled rows; the empty state is the web's.
fn docs_body<'a>(p: &'a Project) -> Element<'a, Msg> {
    if p.docs.is_empty() {
        return ui::empty(
            "file-text",
            "No documents yet",
            "The brief and the proposal appear here once the orchestrator drafts them.",
        );
    }
    let mut docs: Vec<&'a Doc> = p.docs.iter().collect();
    docs.sort_by_key(|d| rank(&d.kind));
    column(
        docs.into_iter()
            .enumerate()
            .map(|(i, d)| ruled(i == 0, doc_item(d))),
    )
    .into()
}

/// One document (the web's `.pj-docrow`): its icon, its name and kind, who and when, its summary, and the Open
/// button; a repository path sits under it in mono.
fn doc_item<'a>(d: &'a Doc) -> Element<'a, Msg> {
    let p = crate::theme::palette();
    let where_ = d.repo_path.as_ref().map(|path| {
        let commit = d
            .commit
            .as_deref()
            .map(|c| format!(" @ {}", c.get(..7).unwrap_or(c)))
            .unwrap_or_default();
        format!("{path}{commit}")
    });
    let icon_box = container(ui::icon("file-text", 16.0, p.ink_3))
        .padding(8)
        .style(move |_: &Theme| container::Style {
            background: Some(crate::theme::palette().sheet_2.into()),
            border: Border::default().rounded(crate::theme::skin().radius),
            ..Default::default()
        });
    let mut main = column![
        row![
            semibold(d.title.as_str(), 14.0),
            ui::badge(&d.kind, ui::Tone::Muted)
        ]
        .spacing(8)
        .align_y(Alignment::Center),
        ui::sans(format!("by {} · {}", d.by, ago(d.at_ms)), 12.0, p.ink_3),
    ]
    .spacing(4)
    .width(Length::Fill);
    if !d.summary.is_empty() {
        main = main.push(ui::sans(d.summary.as_str(), 13.0, p.ink_2));
    }
    if let Some(w) = where_ {
        main = main.push(ui::mono(w, 12.0, p.ink_3));
    }
    let mut line = row![icon_box, main].spacing(12).align_y(Alignment::Center);
    if let Some(id) = &d.file {
        line = line.push(ui::btn_sm(
            "Open",
            None,
            ui::Kind::Ghost,
            Some(Msg::OpenFile(id.clone())),
        ));
    }
    container(line).padding([12, 0]).width(Length::Fill).into()
}

fn critiques_body<'a>(p: &'a Project) -> Element<'a, Msg> {
    if p.critiques.is_empty() {
        return ui::empty(
            "messages-square",
            "No critiques yet",
            "The critiques appear here once agents review the proposal.",
        );
    }
    column(
        p.critiques
            .iter()
            .enumerate()
            .map(|(i, c)| ruled(i == 0, critique_item(c))),
    )
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
        semibold(format!("by {}", c.by), 13.0),
        ui::sans(
            format!("revision {}", c.revision),
            12.0,
            crate::theme::palette().ink_3
        ),
        ui::sans(ago(c.at_ms), 12.0, crate::theme::palette().ink_3),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let mut col = column![head].spacing(8);
    if !c.text.is_empty() {
        col = col.push(ui::markdown(&c.text));
    }
    container(col).padding([12, 0]).width(Length::Fill).into()
}

fn tasks_body<'a>(p: &'a Project) -> Element<'a, Msg> {
    if p.tasks.is_empty() {
        return ui::faint("No tasks yet.").into();
    }
    column![
        column(p.tasks.iter().enumerate().map(|(i, t)| {
            ruled(
                i == 0,
                container(
                    row![
                        ui::mono(t.as_str(), 12.0, crate::theme::palette().ink_3),
                        link(t, Msg::Go(Id::Task(t.clone()))),
                    ]
                    .spacing(12)
                    .align_y(Alignment::Center),
                )
                .padding([10, 0])
                .width(Length::Fill)
                .into(),
            )
        })),
        link(
            "See them in the order they run, in Work",
            Msg::Go(Id::WorkOn(p.id.clone()))
        ),
    ]
    .spacing(8)
    .into()
}

fn history_body<'a>(p: &'a Project) -> Element<'a, Msg> {
    if p.history.is_empty() {
        return ui::faint("Nothing recorded yet.").into();
    }
    let mut events: Vec<&'a Event> = p.history.iter().collect();
    events.sort_by_key(|e| e.at_ms);
    column(events.into_iter().enumerate().map(|(i, e)| {
        ruled(
            i == 0,
            container(event_row(e))
                .padding([8, 0])
                .width(Length::Fill)
                .into(),
        )
    }))
    .into()
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
    Subscription::batch([
        events::subscription(&ctx.client).map(Msg::Event),
        // Files dropped on the window while a project is open go into its repository.
        iced::event::listen_with(|event, _, _| match event {
            iced::Event::Window(iced::window::Event::FileDropped(path)) => {
                Some(Msg::FilesPicked(vec![path]))
            }
            _ => None,
        }),
    ])
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
