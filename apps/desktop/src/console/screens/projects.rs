//! The Projects screen (parity with the web console's `projects` view).
//!
//! The list of goals, and one project's page: its brief, the eight-stage track, documents,
//! critiques, tasks, repository and history. The owner approves the plan or sends the project
//! back to an earlier stage with a reason.

use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{button, column, container, row, text_input, Space};
use iced::{Alignment, Element, Length, Task, Theme};
use serde::Deserialize;
use serde_json::json;

use super::Id;
use crate::console::{client, ui, Ctx};
use crate::theme;

/// The eight stages, in order (`PROJECT_STAGES` on the web).
const STAGES: [&str; 8] = ["brief", "research", "proposal", "critique", "approval", "implementation", "review", "delivered"];
/// The stages in which the owner can approve the plan.
const APPROVABLE: [&str; 2] = ["critique", "approval"];

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
}

/// A document a stage produced.
#[derive(Debug, Clone, Deserialize)]
pub struct Doc {
    pub kind: String,
    pub title: String,
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
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    if let Msg::Go(id) = msg {
        Some(id.clone())
    } else {
        None
    }
}

/// Loads the screen's data (called when it opens and on every poll tick).
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    match state.open.clone() {
        Some(id) => fetch_detail(ctx, id),
        None => fetch_list(ctx),
    }
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::List(res) => {
            match res {
                Ok(list) => {
                    state.list = Some(list);
                    state.error = None;
                }
                Err(e) => state.error = Some(e),
            }
            Task::none()
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
            Task::none()
        }
        Msg::Open(id) => {
            state.open = Some(id.clone());
            state.detail = None;
            state.notice = None;
            state.approve = None;
            state.send_back = None;
            state.error = None;
            fetch_detail(ctx, id)
        }
        Msg::Back => {
            state.open = None;
            state.detail = None;
            state.notice = None;
            state.approve = None;
            state.send_back = None;
            state.error = None;
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
            let Some(p) = state.detail.as_ref() else { return Task::none() };
            let idx = stage_index(&p.stage);
            if idx == 0 {
                return Task::none();
            }
            state.approve = None;
            state.send_back = Some(SendBack { stage: STAGES[idx - 1].to_string(), reason: String::new() });
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
            let Some(p) = state.detail.as_ref() else { return Task::none() };
            let Some(note) = state.approve.as_ref() else { return Task::none() };
            let client = ctx.client.clone();
            let path = format!("/v1/projects/{}/approve", p.id);
            let body = json!({ "note": note.trim() });
            client::task(move || client.post::<Project>(&path, &body), |r| Msg::Acted(r, "Plan approved"))
        }
        Msg::SubmitSendBack => {
            let Some(p) = state.detail.as_ref() else { return Task::none() };
            let Some(sb) = state.send_back.as_ref() else { return Task::none() };
            let why = sb.reason.trim().to_string();
            if why.is_empty() {
                state.error = Some("Give a reason so the team knows what to change".into());
                return Task::none();
            }
            let client = ctx.client.clone();
            let path = format!("/v1/projects/{}/veto", p.id);
            let body = json!({ "stage": sb.stage, "reason": why });
            client::task(move || client.post::<Project>(&path, &body), |r| Msg::Acted(r, "Sent back"))
        }
    }
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    match (&state.open, &state.detail) {
        (None, _) => list_view(state),
        (Some(_), Some(p)) => detail(state, p),
        (Some(_), None) => {
            let body: Element<'a, Msg> = match &state.error {
                Some(e) => ui::error_line(e),
                None => ui::loading(),
            };
            ui::sheet(ui::view_head("Project", vec![]), column![ui::btn("← All projects", None, ui::Kind::Ghost, Some(Msg::Back)), body].spacing(18).into())
        }
    }
}

fn fetch_list(ctx: &Ctx) -> Task<Msg> {
    let client = ctx.client.clone();
    client::task(move || client.get::<Vec<Project>>("/v1/projects"), Msg::List)
}

fn fetch_detail(ctx: &Ctx, id: String) -> Task<Msg> {
    let client = ctx.client.clone();
    let path = format!("/v1/projects/{id}");
    client::task(move || client.get::<Project>(&path), move |r| Msg::Detail(id.clone(), r))
}

// ---- helpers -------------------------------------------------------------------------

fn stage_index(stage: &str) -> usize {
    STAGES.iter().position(|s| *s == stage).unwrap_or(0)
}

/// Sort rank of a document kind: its stage's place, unknown kinds last.
fn rank(kind: &str) -> usize {
    STAGES.iter().position(|s| *s == kind).unwrap_or(STAGES.len())
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
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    let s = (now - ms).max(0) / 1000;
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

/// The eight segments, filled up to the current stage, with the stage name beside them.
fn track<'a>(stage: &str) -> Element<'a, Msg> {
    let idx = stage_index(stage);
    let segs = STAGES.iter().enumerate().fold(row![].spacing(3), |r, (i, _)| {
        let color = if i == idx {
            theme::SIGNAL
        } else if i < idx {
            theme::INK
        } else {
            theme::RULE_FAINT
        };
        r.push(
            container(Space::new().width(Length::Fill).height(6.0))
                .width(Length::Fill)
                .style(move |_: &Theme| container::Style { background: Some(color.into()), ..Default::default() }),
        )
    });
    row![segs.width(Length::Fill), ui::mono(label(stage), 11.0, theme::INK_2)].spacing(10).align_y(Alignment::Center).into()
}

/// The pipeline on the detail page: done steps are ticked, the current one is highlighted, later ones faint.
fn pipeline<'a>(stage: &str) -> Element<'a, Msg> {
    let idx = stage_index(stage);
    let steps = STAGES.iter().enumerate().fold(row![].spacing(6), |r, (i, s)| {
        let (mark, color) = if i < idx {
            ("✓".to_string(), theme::OK)
        } else if i == idx {
            ((i + 1).to_string(), theme::SIGNAL)
        } else {
            ((i + 1).to_string(), theme::INK_3)
        };
        r.push(column![ui::mono(mark, 11.0, color), ui::mono(label(s), 11.0, color)].spacing(4).width(Length::Fill))
    });
    steps.into()
}

// ---- the list ------------------------------------------------------------------------

fn row_item<'a>(p: &'a Project) -> Element<'a, Msg> {
    let main = column![
        row![ui::mono(p.title.as_str(), 14.0, theme::INK), ui::badge(&p.kind, ui::Tone::Muted)].spacing(10).align_y(Alignment::Center),
        track(&p.stage),
        ui::faint(format!("rev {} · {} docs · {} critiques · {} tasks", p.revision, p.docs.len(), p.critiques.len(), p.tasks.len())),
    ]
    .spacing(8)
    .width(Length::Fill);
    let side = ui::faint(format!("updated {}", ago(p.updated_ms)));
    button(row![main, side].spacing(16).align_y(Alignment::Center))
        .on_press(Msg::Open(p.id.clone()))
        .padding(14)
        .width(Length::Fill)
        .style(theme::register_row(false))
        .into()
}

fn list_view<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut body: Vec<Element<'a, Msg>> = vec![container(ui::prose(
        "Every goal goes through brief, research, proposal, critique and approval before anything is built.",
    ))
    .max_width(620)
    .into()];
    if let Some(e) = &state.error {
        body.push(ui::error_line(e));
    }
    match &state.list {
        None => body.push(ui::loading()),
        Some(list) if list.is_empty() => {
            body.push(ui::empty("projects", "No projects yet.", "The orchestrator opens one for each goal you give it."))
        }
        Some(list) => body.push(column(list.iter().map(row_item)).spacing(10).into()),
    }
    ui::sheet(ui::view_head("Projects", vec![]), column(body).spacing(18).into())
}

// ---- a project's page ----------------------------------------------------------------

fn detail<'a>(state: &'a State, p: &'a Project) -> Element<'a, Msg> {
    let idx = stage_index(&p.stage);

    let mut badges: Vec<Element<'a, Msg>> = vec![ui::badge(&p.kind, ui::Tone::Muted), ui::badge(&label(&p.stage), ui::Tone::Signal)];
    if let Some(by) = &p.approved_by {
        badges.push(ui::faint(format!("approved by {by}")).into());
    }

    let mut body: Vec<Element<'a, Msg>> = vec![
        ui::btn("← All projects", None, ui::Kind::Ghost, Some(Msg::Back)),
        ui::prose(p.goal.as_str()).into(),
        pipeline(&p.stage),
    ];

    let mut actions: Vec<Element<'a, Msg>> = Vec::new();
    if APPROVABLE.contains(&p.stage.as_str()) {
        actions.push(ui::btn("Approve plan", None, ui::Kind::Primary, Some(Msg::OpenApprove)));
    }
    if idx > 0 {
        actions.push(ui::btn("Send back…", None, ui::Kind::Secondary, Some(Msg::OpenSendBack)));
    }
    if !actions.is_empty() {
        body.push(row(actions).spacing(8).into());
    }

    if let Some(note) = &state.approve {
        let field = text_input("A note for the team (optional)", note)
            .on_input(Msg::ApproveNote)
            .on_submit(Msg::SubmitApprove)
            .style(theme::input);
        let form = column![
            field,
            row![
                ui::btn("Approve plan", None, ui::Kind::Primary, Some(Msg::SubmitApprove)),
                ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CancelForm)),
            ]
            .spacing(8),
        ]
        .spacing(10);
        body.push(ui::card("Approve plan", form.into()));
    }

    if let Some(sb) = &state.send_back {
        let picks = row(STAGES[..idx].iter().map(|s| {
            let kind = if *s == sb.stage { ui::Kind::Primary } else { ui::Kind::Secondary };
            ui::btn(&label(s), None, kind, Some(Msg::PickStage(s.to_string())))
        }))
        .spacing(6);
        let reason = text_input("What must change, and why", &sb.reason)
            .on_input(Msg::Reason)
            .on_submit(Msg::SubmitSendBack)
            .style(theme::input);
        let form = column![
            ui::faint("Back to"),
            picks,
            reason,
            ui::btn("Send back", None, ui::Kind::Danger, Some(Msg::SubmitSendBack)),
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

    let mut sections: Vec<Element<'a, Msg>> = vec![
        ui::card("Documents", docs_body(p)),
        ui::card("Critiques", critiques_body(p)),
        ui::card("Tasks", tasks_body(p)),
    ];
    if let Some(repo) = &p.repo {
        sections.push(ui::card("Repository", ui::btn(repo, None, ui::Kind::Ghost, Some(Msg::Go(Id::Git)))));
    }
    sections.push(ui::card("History", history_body(p)));
    body.push(column(sections).spacing(16).into());

    ui::sheet(ui::view_head(&p.title, badges), column(body).spacing(18).into())
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
        row![ui::prose(d.title.as_str()), ui::badge(&d.kind, ui::Tone::Muted)].spacing(8).align_y(Alignment::Center),
        ui::faint(format!("by {} · {}", d.by, ago(d.at_ms))),
    ]
    .spacing(6);
    if !d.summary.is_empty() {
        col = col.push(ui::markdown(&d.summary));
    }
    if let Some(path) = &d.repo_path {
        let commit = d.commit.as_deref().map(|c| format!(" @ {}", c.get(..7).unwrap_or(c))).unwrap_or_default();
        col = col.push(ui::mono(format!("{path}{commit}"), 11.0, theme::INK_2));
    }
    col.into()
}

fn critiques_body<'a>(p: &'a Project) -> Element<'a, Msg> {
    if p.critiques.is_empty() {
        return ui::faint("No critiques yet.").into();
    }
    column(p.critiques.iter().map(critique_item)).spacing(16).into()
}

fn critique_item<'a>(c: &'a Critique) -> Element<'a, Msg> {
    let tone = match c.verdict.as_str() {
        "approve" => ui::Tone::Ok,
        "revise" => ui::Tone::Danger,
        _ => ui::Tone::Muted,
    };
    let head = row![
        ui::badge(&c.verdict, tone),
        ui::faint(format!("by {}", c.by)),
        ui::faint(format!("revision {}", c.revision)),
        ui::faint(ago(c.at_ms)),
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
    column(p.tasks.iter().map(|t| ui::btn(t, None, ui::Kind::Ghost, Some(Msg::Go(Id::Tasks))))).spacing(6).into()
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
        ui::faint(ago(e.at_ms)),
        ui::mono(e.by.as_str(), 12.0, theme::INK),
        ui::prose(e.what.as_str()),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    if !e.note.is_empty() {
        line = line.push(ui::faint(format!("— {}", e.note)));
    }
    line.into()
}
