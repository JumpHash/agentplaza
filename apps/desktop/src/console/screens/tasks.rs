//! The Tasks screen (parity with the web console's `tasks` view): every task on a four-lane
//! board, and one task in full (goal, acceptance, result, facts, the assignee's timeline).
//! Posting a task, assigning it and withdrawing it are the actions the web offers. The screen
//! polls every 4 s; elapsed times are drawn at each poll.

use std::collections::HashMap;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, mouse_area, pick_list, row, scrollable, text, text_editor, text_input, Space};
use iced::{font, Alignment, Border, Color, Element, Font, Length, Padding, Task, Theme};
use serde::Deserialize;
use serde_json::{json, Value};

use super::Id;
use crate::console::ui::{self, Tone};
use crate::console::{client, Ctx};
use crate::theme::{self, CRIT, OK};

/// A card's goal: serif semibold, as the web's task title.
const TITLE_FONT: Font = Font { weight: font::Weight::Semibold, ..theme::SERIF };
/// Bold mono, for lane counts and the elapsed time.
const MONO_BOLD: Font = Font { weight: font::Weight::Bold, ..theme::MONO };
/// Serif italic, for the line an empty lane shows.
const ITALIC: Font = Font { style: font::Style::Italic, ..theme::SERIF };

/// The four lanes: their label and the line an empty lane shows.
const LANES: [(&str, &str); 4] = [
    ("Open", "Nothing open."),
    ("In progress", "Nothing in progress."),
    ("Done", "Nothing finished yet."),
    ("Failed or withdrawn", "Nothing failed or withdrawn."),
];

/// The transcript entries that make up a task's timeline.
const TIMELINE_KINDS: [&str; 4] = ["work_start", "tool_request", "work_end", "error"];

#[derive(Debug, Clone, Deserialize)]
pub struct TaskRecord {
    pub id: String,
    pub state: String,
    pub goal: String,
    #[serde(default)]
    pub acceptance: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub budget: f64,
    #[serde(default)]
    pub agent_cid: Option<u64>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentRecord {
    pub cid: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub role_label: String,
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EntryRecord {
    pub at_ms: i64,
    pub work: u64,
    pub kind: String,
    #[serde(default)]
    pub data: Value,
}

#[derive(Deserialize)]
struct TasksReply {
    tasks: Vec<TaskRecord>,
}

/// One task in full: the task (None when it cannot be read) and its assignee's timeline.
#[derive(Debug, Clone)]
pub struct Detail {
    task: Option<TaskRecord>,
    timeline: Vec<EntryRecord>,
}

/// What one poll reads: the board (or the open task), the agents, and the live activity lines.
#[derive(Debug, Clone)]
pub struct Snapshot {
    tasks: Vec<TaskRecord>,
    agents: Vec<AgentRecord>,
    activity: Vec<(u64, String)>,
    detail: Option<Detail>,
}

/// A choice in a pick list: the value it stands for (`key`) and the text it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    key: String,
    label: String,
}

impl Pick {
    fn any_role() -> Pick {
        Pick { key: String::new(), label: "Any".into() }
    }

    fn scheduler() -> Pick {
        Pick { key: String::new(), label: "Let the scheduler choose".into() }
    }
}

impl fmt::Display for Pick {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

/// The new-task form, as the web's modal holds it.
#[derive(Debug)]
struct Draft {
    title: String,
    details: text_editor::Content,
    acceptance: text_editor::Content,
    role: Pick,
    budget: String,
    assignee: Pick,
    posting: bool,
}

impl Draft {
    fn new() -> Draft {
        Draft {
            title: String::new(),
            details: text_editor::Content::new(),
            acceptance: text_editor::Content::new(),
            role: Pick::any_role(),
            budget: "100".into(),
            assignee: Pick::scheduler(),
            posting: false,
        }
    }
}

#[derive(Debug, Default)]
pub struct State {
    pub error: Option<String>,
    selected: Option<String>,
    tasks: Vec<TaskRecord>,
    agents: Vec<AgentRecord>,
    activity: HashMap<u64, String>,
    detail: Option<Detail>,
    draft: Option<Draft>,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Result<Snapshot, String>),
    /// Open one task in full.
    Open(String),
    /// Back to the board.
    Back,
    NewTask,
    CloseForm,
    FormTitle(String),
    FormDetails(text_editor::Action),
    FormAcceptance(text_editor::Action),
    FormRole(Pick),
    FormBudget(String),
    FormAssignee(Pick),
    Submit,
    Created(Result<TaskRecord, String>),
    Assign(Pick),
    Assigned(Result<Value, String>),
    Withdraw,
    Withdrawn(Result<Value, String>),
}

/// The navigation this message asks for, if any.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// Loads the screen's data (called when it opens and on every poll tick).
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    fetch(ctx, state.selected.clone())
}

pub fn update(state: &mut State, msg: Msg, ctx: &Ctx) -> Task<Msg> {
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(Ok(snap)) => {
            state.error = None;
            if snap.detail.is_none() {
                state.tasks = snap.tasks;
                state.activity = snap.activity.into_iter().collect();
            }
            state.agents = snap.agents;
            state.detail = if state.selected.is_some() { snap.detail } else { None };
            Task::none()
        }
        Msg::Loaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::Open(id) => {
            state.selected = Some(id.clone());
            state.detail = None;
            fetch(ctx, Some(id))
        }
        Msg::Back => {
            state.selected = None;
            state.detail = None;
            fetch(ctx, None)
        }
        Msg::NewTask => {
            state.draft = Some(Draft::new());
            Task::none()
        }
        Msg::CloseForm => {
            state.draft = None;
            Task::none()
        }
        Msg::FormTitle(v) => {
            if let Some(d) = state.draft.as_mut() {
                d.title = v;
            }
            Task::none()
        }
        Msg::FormDetails(a) => {
            if let Some(d) = state.draft.as_mut() {
                d.details.perform(a);
            }
            Task::none()
        }
        Msg::FormAcceptance(a) => {
            if let Some(d) = state.draft.as_mut() {
                d.acceptance.perform(a);
            }
            Task::none()
        }
        Msg::FormRole(p) => {
            if let Some(d) = state.draft.as_mut() {
                d.role = p;
            }
            Task::none()
        }
        Msg::FormBudget(v) => {
            if let Some(d) = state.draft.as_mut() {
                d.budget = v;
            }
            Task::none()
        }
        Msg::FormAssignee(p) => {
            if let Some(d) = state.draft.as_mut() {
                d.assignee = p;
            }
            Task::none()
        }
        Msg::Submit => submit(state, ctx),
        Msg::Created(Ok(t)) => {
            state.draft = None;
            state.error = None;
            state.selected = Some(t.id.clone());
            state.detail = None;
            fetch(ctx, Some(t.id))
        }
        Msg::Created(Err(e)) => {
            if let Some(d) = state.draft.as_mut() {
                d.posting = false;
            }
            state.error = Some(e);
            Task::none()
        }
        Msg::Assign(p) => {
            let (Some(id), Ok(cid)) = (state.selected.clone(), p.key.parse::<u64>()) else {
                return Task::none();
            };
            let hub = ctx.client.clone();
            client::task(move || hub.post::<Value>(&format!("/v1/agents/{cid}/assign"), &json!({ "task_id": id })), Msg::Assigned)
        }
        Msg::Assigned(done) => settled(state, ctx, done),
        Msg::Withdraw => {
            let Some(id) = state.selected.clone() else {
                return Task::none();
            };
            let hub = ctx.client.clone();
            client::task(move || hub.delete(&format!("/v1/tasks/{id}")), Msg::Withdrawn)
        }
        Msg::Withdrawn(done) => settled(state, ctx, done),
    }
}

/// Validates the form and posts the task; the web's `createTask` body, field for field.
fn submit(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let Some(d) = state.draft.as_ref() else {
        return Task::none();
    };
    if d.posting || d.title.trim().is_empty() {
        return Task::none();
    }
    let budget_text = d.budget.trim().to_string();
    let budget = if budget_text.is_empty() { Ok(100.0) } else { budget_text.parse::<f64>() };
    let Ok(budget) = budget else {
        state.error = Some("Budget must be a number.".into());
        return Task::none();
    };
    let title = d.title.trim().to_string();
    let details = d.details.text().trim().to_string();
    let acceptance = d.acceptance.text().trim().to_string();
    let goal = if details.is_empty() { title } else { format!("{title}\n\n{details}") };
    let mut body = json!({ "goal": goal, "budget": budget });
    if !acceptance.is_empty() {
        body["acceptance"] = json!(acceptance);
    }
    if !d.role.key.is_empty() {
        body["role"] = json!(d.role.key);
    }
    if let Ok(cid) = d.assignee.key.parse::<u64>() {
        body["assignee"] = json!(cid);
    }
    if let Some(d) = state.draft.as_mut() {
        d.posting = true;
    }
    let hub = ctx.client.clone();
    client::task(move || hub.post::<TaskRecord>("/v1/tasks", &body), Msg::Created)
}

/// After an assign or a withdraw: reload on success, show the error otherwise.
fn settled(state: &mut State, ctx: &Ctx, done: Result<Value, String>) -> Task<Msg> {
    match done {
        Ok(_) => {
            state.error = None;
            fetch(ctx, state.selected.clone())
        }
        Err(e) => {
            state.error = Some(e);
            Task::none()
        }
    }
}

/// One poll: the agents, then either the board (tasks and the live activity of the in-progress
/// assignees) or the open task and its timeline. Runs off the UI thread.
fn fetch(ctx: &Ctx, selected: Option<String>) -> Task<Msg> {
    let hub = ctx.client.clone();
    client::task(move || snapshot(&hub, selected), Msg::Loaded)
}

fn snapshot(hub: &client::Client, selected: Option<String>) -> Result<Snapshot, String> {
    let agents: Vec<AgentRecord> = hub.get("/v1/agents")?;
    if let Some(id) = selected {
        let task: Option<TaskRecord> = hub.get(&format!("/v1/tasks/{id}")).ok();
        let timeline = match task.as_ref().and_then(|t| t.agent_cid) {
            Some(cid) => timeline_of(&id, &transcript(hub, cid)),
            None => Vec::new(),
        };
        return Ok(Snapshot { tasks: Vec::new(), agents, activity: Vec::new(), detail: Some(Detail { task, timeline }) });
    }
    let tasks = hub.get::<TasksReply>("/v1/tasks")?.tasks;
    let mut activity: Vec<(u64, String)> = Vec::new();
    for cid in tasks.iter().filter(|t| lane_of(&t.state) == 1).filter_map(|t| t.agent_cid) {
        if activity.iter().any(|(c, _)| *c == cid) {
            continue;
        }
        if let Some(line) = transcript(hub, cid).iter().rev().find_map(activity_of) {
            activity.push((cid, line));
        }
    }
    Ok(Snapshot { tasks, agents, activity, detail: None })
}

fn transcript(hub: &client::Client, cid: u64) -> Vec<EntryRecord> {
    hub.get(&format!("/v1/agents/{cid}/transcript?after=0&limit=1000")).unwrap_or_default()
}

/// The lane for a state: open, in progress, done, or failed or withdrawn; unknown non-final
/// states count as in progress.
fn lane_of(state: &str) -> usize {
    match state {
        "open" => 0,
        "done" => 2,
        "failed" | "withdrawn" | "cancelled" => 3,
        _ => 1,
    }
}

fn lane_tone(lane: usize) -> Tone {
    match lane {
        0 => Tone::Info,
        1 => Tone::Signal,
        2 => Tone::Ok,
        _ => Tone::Danger,
    }
}

fn tone_of_state(state: &str) -> Tone {
    match lane_of(state) {
        3 if state == "withdrawn" => Tone::Muted,
        lane => lane_tone(lane),
    }
}

fn label_of(state: &str) -> String {
    let s = state.replace('_', " ");
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn title_of(goal: &str) -> String {
    goal.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("Untitled task").to_string()
}

/// Text for a card, held to about `n` characters: whitespace is flattened, the cut falls at a
/// word boundary where there is one, and an ellipsis ends it. The full text stays in the task page.
fn clip(s: &str, n: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= n {
        return flat;
    }
    let cut: String = flat.chars().take(n).collect();
    let head = match cut.rfind(' ') {
        Some(i) if i * 2 > n => &cut[..i],
        _ => cut.as_str(),
    };
    format!("{}…", head.trim_end())
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// "5m ago" style, relative to now.
fn ago(ms: i64) -> String {
    let s = (now_ms() - ms).max(0) / 1000;
    match s {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

/// mm:ss, the elapsed time of work that is running.
fn fmt_elapsed(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// "4m 12s", the length of a finished task.
fn fmt_span(ms: i64) -> String {
    let s = ((ms as f64) / 1000.0).round().max(0.0) as i64;
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{}h {:02}m", s / 3600, (s % 3600) / 60)
    }
}

/// HH:MM:SS of a timestamp, in UTC.
fn clock(ms: i64) -> String {
    let s = ms.div_euclid(1000).rem_euclid(86400);
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

fn fmt_num(v: f64) -> String {
    format!("{v}")
}

fn tool_of(e: &EntryRecord) -> String {
    str_field(&e.data, "tool").or_else(|| str_field(&e.data, "name")).unwrap_or("tool").to_string()
}

fn str_field<'v>(v: &'v Value, key: &str) -> Option<&'v str> {
    v.get(key).and_then(Value::as_str)
}

/// The shell command of a bash tool request, or None for any other tool.
fn command_of(e: &EntryRecord) -> Option<String> {
    if tool_of(e).to_lowercase() != "bash" {
        return None;
    }
    e.data.get("input").and_then(|i| str_field(i, "command")).map(str::to_string)
}

/// What one transcript entry says the agent is doing now, or None when it says nothing new.
fn activity_of(e: &EntryRecord) -> Option<String> {
    match e.kind.as_str() {
        "tool_request" => Some(match command_of(e) {
            Some(cmd) => format!("$ {cmd}"),
            None => format!("→ {}", tool_of(e)),
        }),
        "model_request" => Some("thinking…".to_string()),
        "work_start" => Some("starting…".to_string()),
        "work_end" => Some("wrapping up…".to_string()),
        _ => None,
    }
}

/// One timeline line for a transcript entry.
fn describe(e: &EntryRecord) -> String {
    match e.kind.as_str() {
        "work_start" => format!("Started · {}", str_field(&e.data, "kind").unwrap_or("work")),
        "tool_request" => match command_of(e) {
            Some(cmd) => format!("$ {cmd}"),
            None => format!("→ {}", tool_of(e)),
        },
        "work_end" => match str_field(&e.data, "status") {
            Some(status) if !status.is_empty() => format!("Finished · {status}"),
            _ => "Finished".to_string(),
        },
        _ => str_field(&e.data, "message")
            .or_else(|| str_field(&e.data, "error"))
            .filter(|s| !s.is_empty())
            .unwrap_or("Error")
            .to_string(),
    }
}

/// The entries of one task's work: the work_start entries that name the task, and the steps of
/// their work.
fn timeline_of(task_id: &str, entries: &[EntryRecord]) -> Vec<EntryRecord> {
    let is_task = |e: &EntryRecord| {
        let origin = e.data.get("origin").and_then(|o| str_field(o, "task"));
        origin == Some(task_id) || str_field(&e.data, "task") == Some(task_id)
    };
    let works: Vec<u64> = entries.iter().filter(|e| e.kind == "work_start" && is_task(e)).map(|e| e.work).collect();
    entries
        .iter()
        .filter(|e| works.contains(&e.work) && TIMELINE_KINDS.contains(&e.kind.as_str()))
        .cloned()
        .collect()
}

fn role_label(agents: &[AgentRecord], role: &str) -> String {
    agents
        .iter()
        .find(|a| a.role == role && !a.role_label.is_empty())
        .map(|a| a.role_label.clone())
        .unwrap_or_else(|| role.to_string())
}

fn agent_name(agents: &[AgentRecord], cid: u64) -> String {
    match agents.iter().find(|a| a.cid == cid) {
        Some(a) if a.name.trim().is_empty() => "Unnamed".to_string(),
        Some(a) => a.name.clone(),
        None => format!("agent {cid}"),
    }
}

fn status_of(agents: &[AgentRecord], cid: Option<u64>) -> String {
    cid.and_then(|c| agents.iter().find(|a| a.cid == c))
        .map(|a| a.status.clone())
        .unwrap_or_else(|| "offline".to_string())
}

fn status_color(status: &str) -> Color {
    match status {
        "working" => ui::SIGNAL,
        "idle" => OK,
        _ => ui::INK_3,
    }
}

fn idle_workers(agents: &[AgentRecord]) -> Vec<&AgentRecord> {
    agents.iter().filter(|a| a.status == "idle" && a.tier == "worker").collect()
}

/// Every worker role on the hub, sorted, with "Any" first.
fn role_options(agents: &[AgentRecord]) -> Vec<Pick> {
    let mut roles: Vec<String> = agents.iter().filter(|a| a.tier == "worker" && !a.role.is_empty()).map(|a| a.role.clone()).collect();
    roles.sort();
    roles.dedup();
    let mut options = vec![Pick::any_role()];
    options.extend(roles.into_iter().map(|r| Pick { label: role_label(agents, &r), key: r }));
    options
}

pub fn view<'a>(state: &'a State, _ctx: &'a Ctx) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let Some(e) = state.error.as_deref() {
        parts.push(ui::error_line(e));
    }
    let mut actions: Vec<Element<'a, Msg>> = Vec::new();
    if state.selected.is_some() {
        parts.push(detail_page(state));
    } else {
        actions.push(ui::btn("New task", Some("plus"), ui::Kind::Primary, Some(Msg::NewTask)));
        if let Some(d) = state.draft.as_ref() {
            parts.push(draft_form(state, d));
        }
        parts.push(summary(state));
        parts.push(board(state));
    }
    // The page scrolls under its head: the lanes are as tall as their cards, not the window.
    // The right padding keeps the scrollbar clear of the last lane.
    let page = column(parts).spacing(22).padding(Padding { top: 0.0, right: 14.0, bottom: 0.0, left: 0.0 });
    ui::sheet(ui::view_head("Tasks", actions), scrollable(page).into())
}

/// The head strip: a figure for each lane, and how many agents are working now.
fn summary<'a>(state: &'a State) -> Element<'a, Msg> {
    let count = |lane: usize| state.tasks.iter().filter(|t| lane_of(&t.state) == lane).count();
    let working = state.agents.iter().filter(|a| a.status == "working").count();
    row![
        fig("Open", count(0)),
        fig("In progress", count(1)),
        fig("Done", count(2)),
        fig("Failed", count(3)),
        fig("Agents working", working),
    ]
    .spacing(28)
    .into()
}

fn fig<'a>(label: &str, value: usize) -> Element<'a, Msg> {
    column![ui::mono(value.to_string(), 22.0, ui::INK), ui::label(label)].spacing(2).into()
}

/// The four lanes, each full height, with a line when empty.
fn board<'a>(state: &'a State) -> Element<'a, Msg> {
    let mut sorted: Vec<&'a TaskRecord> = state.tasks.iter().collect();
    sorted.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    let lanes: Vec<Element<'a, Msg>> = LANES
        .iter()
        .enumerate()
        .map(|(i, (label, idle))| {
            let items: Vec<Element<'a, Msg>> = sorted.iter().filter(|t| lane_of(&t.state) == i).map(|t| task_card(state, *t)).collect();
            let count = items.len();
            let body: Element<'a, Msg> = if items.is_empty() {
                text(*idle).font(ITALIC).size(15.0).color(ui::INK_3).into()
            } else {
                column(items).spacing(10).into()
            };
            lane(i, label, count, body)
        })
        .collect();
    row(lanes).spacing(14).width(Length::Fill).into()
}

fn lane<'a>(i: usize, label: &str, count: usize, body: Element<'a, Msg>) -> Element<'a, Msg> {
    let mark = container(Space::new().width(10.0).height(10.0)).style(theme::swatch(lane_tone(i).color()));
    let head = row![
        mark,
        text(label.to_uppercase()).font(MONO_BOLD).size(11.5).color(ui::INK),
        Space::new().width(Length::Fill),
        text(count.to_string()).font(MONO_BOLD).size(12.0).color(ui::INK_2),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    container(column![head, ui::hairline(), body].spacing(12))
        .padding(12.0)
        .width(Length::FillPortion(1))
        .style(theme::boxed)
        .into()
}

/// One card, drawn as the web draws it: the id and role on top, the goal in serif semibold
/// held to two lines, the assignee, then a footer that depends on the lane. Every line is
/// clipped or wrapped to the card, so no text pushes past its edge.
fn task_card<'a>(state: &'a State, t: &'a TaskRecord) -> Element<'a, Msg> {
    let now = now_ms();
    let status = status_of(&state.agents, t.agent_cid);
    let dot = container(Space::new().width(8.0).height(8.0)).style(theme::swatch(status_color(&status)));
    let who = row![dot, assignee_link(state, t.agent_cid)].spacing(8).align_y(Alignment::Center).width(Length::Fill);
    let role = t.role.as_deref().map(|r| role_label(&state.agents, r)).unwrap_or_else(|| "Any role".to_string());
    let detail: Element<'a, Msg> = match lane_of(&t.state) {
        1 => {
            let line = t
                .agent_cid
                .and_then(|cid| state.activity.get(&cid).cloned())
                .unwrap_or_else(|| "waiting for the first step…".to_string());
            column![
                stripe(),
                container(mono_text(clip(&line, 36), 11.5, ui::INK_2)).width(Length::Fill).clip(true),
                meta_row(meta_text("elapsed".to_string()), ui::mono(fmt_elapsed(now - t.updated_at), 12.0, ui::INK).into()),
            ]
            .spacing(8)
            .width(Length::Fill)
            .into()
        }
        2 => {
            let mut items: Vec<Element<'a, Msg>> = Vec::new();
            if let Some(note) = t.note.as_deref() {
                items.push(serif_text(clip(note, 110), 13.5, ui::INK_2));
            }
            items.push(meta_row(
                meta_text(format!("took {}", fmt_span(t.updated_at - t.created_at))),
                meta_text(ago(t.updated_at)),
            ));
            column(items).spacing(8).width(Length::Fill).into()
        }
        3 => {
            let reason = t.outcome.clone().or_else(|| t.note.clone()).unwrap_or_else(|| label_of(&t.state));
            column![
                serif_text(clip(&reason, 160), 13.0, CRIT),
                meta_row(meta_text(label_of(&t.state)), meta_text(ago(t.updated_at))),
            ]
            .spacing(8)
            .into()
        }
        _ => meta_row(
            meta_text(format!("budget {}", fmt_num(t.budget))),
            meta_text(format!("created {}", ago(t.created_at))),
        ),
    };
    let top = row![
        mono_text(t.id.clone(), 11.0, ui::INK_3),
        Space::new().width(Length::Fill),
        mono_text(clip(&role, 18).to_uppercase(), 10.5, ui::INK_2),
    ]
    .align_y(Alignment::Center)
    .width(Length::Fill);
    let title = container(
        text(clip(&title_of(&t.goal), 42))
            .font(TITLE_FONT)
            .size(15.0)
            .line_height(1.35)
            .color(ui::INK)
            .wrapping(Wrapping::WordOrGlyph),
    )
    .width(Length::Fill)
    .height(Length::Fixed(41.0))
    .clip(true);
    let body = column![top, title, who, container(detail).width(Length::Fill)].spacing(10).width(Length::Fill);
    mouse_area(container(body).padding(12.0).width(Length::Fill).style(card_box))
        .on_press(Msg::Open(t.id.clone()))
        .into()
}

/// The card's box: the paper colour under a rule, as the web's task card.
fn card_box(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(ui::INK),
        background: Some(theme::PAPER.into()),
        border: Border { color: theme::RULE, width: 1.0, radius: 0.0.into() },
        ..container::Style::default()
    }
}

/// A solid fill with no border, for the running stripe.
fn solid(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme: &Theme| container::Style { background: Some(color.into()), ..container::Style::default() }
}

/// The orange dashed rule under a running card; the web's stripe, drawn still and clipped.
fn stripe<'a>() -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    for _ in 0..9 {
        parts.push(container(Space::new().width(10.0).height(3.0)).style(solid(ui::SIGNAL)).into());
        parts.push(Space::new().width(10.0).height(3.0).into());
    }
    container(row(parts)).width(Length::Fill).clip(true).into()
}

/// Mono text, for the card's labels and figures.
fn mono_text<'a>(s: String, size: f32, color: Color) -> Element<'a, Msg> {
    text(s).font(theme::MONO).size(size).color(color).wrapping(Wrapping::None).into()
}


/// The faint mono of a card's footer.
fn meta_text<'a>(s: String) -> Element<'a, Msg> {
    mono_text(s, 11.5, ui::INK_3)
}

/// Serif text that wraps at word boundaries and, failing that, at glyphs, so a long token cannot
/// run past the card.
fn serif_text<'a>(s: String, size: f32, color: Color) -> Element<'a, Msg> {
    text(s).font(theme::SERIF).size(size).color(color).wrapping(Wrapping::WordOrGlyph).into()
}

/// A footer line: the left item at the left edge, the right item at the right edge.
fn meta_row<'a>(left: Element<'a, Msg>, right: Element<'a, Msg>) -> Element<'a, Msg> {
    row![left, Space::new().width(Length::Fill), right].spacing(10).align_y(Alignment::Center).width(Length::Fill).into()
}

/// The assignee's name, which opens the agent; "Unassigned" when there is none.
fn assignee_link<'a>(state: &'a State, cid: Option<u64>) -> Element<'a, Msg> {
    match cid {
        None => ui::faint("Unassigned").into(),
        Some(cid) => button(ui::mono(agent_name(&state.agents, cid), 11.5, ui::INK))
            .padding(0.0)
            .style(button::text)
            .on_press(Msg::Go(Id::Agent(cid)))
            .into(),
    }
}

/// The new-task form, shown above the board.
fn draft_form<'a>(state: &'a State, d: &'a Draft) -> Element<'a, Msg> {
    let title = text_input("What should get done?", &d.title).on_input(Msg::FormTitle).style(theme::input).padding(8.0);
    let details = text_editor(&d.details).placeholder("Details").on_action(Msg::FormDetails).height(120.0);
    let acceptance = text_editor(&d.acceptance).placeholder("Acceptance").on_action(Msg::FormAcceptance).height(80.0);
    let role = pick_list(role_options(&state.agents), Some(d.role.clone()), Msg::FormRole).width(Length::Fill);
    let budget = text_input("100", &d.budget).on_input(Msg::FormBudget).style(theme::input).padding(8.0);
    let mut assignee_options = vec![Pick::scheduler()];
    assignee_options.extend(idle_workers(&state.agents).into_iter().map(|a| Pick {
        key: a.cid.to_string(),
        label: agent_name(&state.agents, a.cid),
    }));
    let assignee = pick_list(assignee_options, Some(d.assignee.clone()), Msg::FormAssignee).width(Length::Fill);
    let cancel = ui::btn("Cancel", None, ui::Kind::Secondary, Some(Msg::CloseForm));
    let submit = ui::btn("Create task", Some("plus"), ui::Kind::Primary, (!d.posting).then_some(Msg::Submit));
    let body = column![
        section("Title", title.into()),
        section("Details", details.into()),
        section("Acceptance", acceptance.into()),
        row![section("Role", role.into()), section("Budget", budget.into())].spacing(14),
        section("Assign to", assignee.into()),
        row![Space::new().width(Length::Fill), cancel, submit].spacing(8).align_y(Alignment::Center),
    ]
    .spacing(14);
    ui::card("New task", body.into())
}

/// One task in full: the goal, acceptance and result, the facts (with assign and withdraw while
/// it is open), and the timeline of its assignee's work.
fn detail_page<'a>(state: &'a State) -> Element<'a, Msg> {
    let back: Element<'a, Msg> = ui::btn("Tasks", Some("chevronRight"), ui::Kind::Ghost, Some(Msg::Back));
    let Some(detail) = state.detail.as_ref() else {
        return column![back, ui::loading()].spacing(14).into();
    };
    let Some(t) = detail.task.as_ref() else {
        return column![back, ui::faint("Task not found.")].spacing(14).into();
    };

    let open = t.state == "open";
    let assign_options: Vec<Pick> = idle_workers(&state.agents)
        .into_iter()
        .map(|a| Pick { key: a.cid.to_string(), label: agent_name(&state.agents, a.cid) })
        .collect();
    let role = t.role.as_deref().map(|r| role_label(&state.agents, r)).unwrap_or_else(|| "Any role".to_string());
    let facts = column![
        kv("State", ui::badge(&label_of(&t.state), tone_of_state(&t.state))),
        kv("Role", ui::faint(role).into()),
        kv("Assignee", assignee_link(state, t.agent_cid)),
        kv("Budget", ui::mono(fmt_num(t.budget), 12.0, ui::INK).into()),
        kv("Created", ui::faint(ago(t.created_at)).into()),
        kv("Updated", ui::faint(ago(t.updated_at)).into()),
    ]
    .spacing(8);
    let mut side_parts: Vec<Element<'a, Msg>> = vec![facts.into()];
    if open {
        side_parts.push(
            pick_list(assign_options, None::<Pick>, Msg::Assign)
                .placeholder("Assign to…")
                .width(Length::Fill)
                .into(),
        );
        side_parts.push(
            row![
                Space::new().width(Length::Fill),
                ui::btn("Withdraw", Some("x"), ui::Kind::Danger, Some(Msg::Withdraw)),
            ]
            .align_y(Alignment::Center)
            .into(),
        );
    }

    let steps: Element<'a, Msg> = if detail.timeline.is_empty() {
        let line = if t.agent_cid.is_none() { "Not yet assigned." } else { "No steps recorded yet." };
        ui::faint(line).into()
    } else {
        column(detail.timeline.iter().map(timeline_row).collect::<Vec<_>>()).spacing(6).into()
    };

    let mut main_parts: Vec<Element<'a, Msg>> = vec![section("Goal", ui::markdown(&t.goal))];
    if let Some(a) = t.acceptance.as_deref().filter(|a| !a.is_empty()) {
        main_parts.push(section("Acceptance", ui::markdown(a)));
    }
    if let Some(n) = t.note.as_deref().filter(|n| !n.is_empty()) {
        main_parts.push(section("Result", ui::markdown(n)));
    }

    let head = column![ui::mono(t.id.clone(), 11.0, ui::INK_3), ui::mono(title_of(&t.goal), 22.0, ui::INK)].spacing(4);
    let main = column(main_parts).spacing(22).width(Length::FillPortion(3));
    let side = column![section("Facts", column(side_parts).spacing(10).into()), section("Timeline", steps)]
        .spacing(22)
        .width(Length::FillPortion(2));
    column![back, head, row![main, side].spacing(28)].spacing(18).into()
}

fn timeline_row<'a>(e: &'a EntryRecord) -> Element<'a, Msg> {
    let color = if e.kind == "error" { CRIT } else { ui::INK };
    row![ui::mono(clock(e.at_ms), 11.0, ui::INK_3), ui::mono(describe(e), 12.0, color)]
        .spacing(12)
        .into()
}

/// A labelled section: a label over its body.
fn section<'a>(title: &str, body: Element<'a, Msg>) -> Element<'a, Msg> {
    column![ui::label(title), body].spacing(10).into()
}

/// A key and its value on one line.
fn kv<'a>(key: &str, value: Element<'a, Msg>) -> Element<'a, Msg> {
    row![container(ui::label(key)).width(Length::Fixed(96.0)), value]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
}

#[cfg(test)]
mod tests {
    use super::clip;

    #[test]
    fn clip_keeps_short_text_and_flattens_whitespace() {
        assert_eq!(clip("  one\n two  ", 20), "one two");
    }

    #[test]
    fn clip_cuts_at_a_word_and_ends_with_an_ellipsis() {
        assert_eq!(clip("RE-ISSUE of task t3 with a workable budget", 20), "RE-ISSUE of task t3…");
        assert!(clip("a0b7ecad718094baff717f1c29be4b2a0b7ecad718094baff", 20).ends_with('…'));
    }
}
