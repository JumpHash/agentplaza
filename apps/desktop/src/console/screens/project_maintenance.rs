//! A project's Maintenance tab (the web's `project-maintenance.ts`): what its watchers keep while it runs in
//! maintenance. The strip with the headline and the sweep; the focus; the map of the product, drawn as a graph with
//! its areas coloured by health and the waiting proposals beside them, and the side panel with the chosen area or
//! proposal; the proposals waiting for the owner, and the decided ones under Expert; the inbox of signals and what the
//! watchers read. The owner's answers are the web's dialogs (Do it, Not this, Add a signal, Sweeps, the ingest
//! address). The mode switch lives on the project's page.

use std::collections::BTreeMap;
use std::f32::consts::PI;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::advanced::text::Alignment as TextAlign;
use iced::widget::canvas::{self, Canvas, Frame, LineDash, Path, Stroke};
use iced::widget::text_editor::Content;
use iced::widget::{
    Space, button, checkbox, column, container, pick_list, row, text, text_editor, text_input,
};
use iced::{
    Alignment, Border, Color, Element, Length, Pixels, Point, Rectangle, Renderer, Task, Theme,
    mouse,
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Id;
use crate::console::{Ctx, client, ui};
use crate::theme;

/// The signals the inbox lists (the newest); the hub keeps 200. Under Expert, all of the newest 200 show.
const SHOWN_SIGNALS: usize = 15;
const SHOWN_SIGNALS_EXPERT: usize = 200;
/// The longest focus the hub takes.
const FOCUS_MAX: usize = 1000;
/// The kinds of signal a person adds, as the hub knows them.
const SIGNAL_KINDS: [&str; 6] = ["feedback", "report", "analytics", "error", "usage", "note"];
/// The intervals the Sweeps dialog offers, in hours: 0 is only when asked.
const EVERY_CHOICES: [i64; 7] = [0, 1, 6, 12, 24, 48, 168];
/// The side of the square the map is drawn in.
const MAP: f32 = 420.0;
/// The side panel's width beside the map.
const SIDE: f32 = 360.0;
/// The proposals the side panel's top list shows.
const TOP_PROPOSALS: usize = 6;
/// The waiting proposals a map area shows as dots (at most).
const DOTS_PER_AREA: usize = 8;
const BUILDING_TITLE: &str = "This project is being built";
const BUILDING: &str = "In maintenance, its agents read its analytics, user reports and feedback, keep a map of it and propose work: what would change, for whom, its value and its risk. Nothing runs until you pick a proposal.";

// ---- The view, as the hub answers it -------------------------------------------------------------------------------

/// `GET /v1/projects/{id}/maintenance`. Every field defaults, so one the hub leaves out never fails the read.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Maintenance {
    pub config: Config,
    /// The agents who sweep, in turn.
    pub watchers: Vec<u64>,
    pub map: ProductMap,
    /// The newest 200, newest first.
    pub signals: Vec<Signal>,
    pub signals_total: i64,
    /// Signals no sweep has read yet.
    pub unseen: i64,
    /// Newest first.
    pub proposals: Vec<Proposal>,
    pub sources: Sources,
    pub sweep: Sweep,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub every_hours: i64,
    pub focus: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProductMap {
    pub summary: String,
    pub areas: Vec<Area>,
}

/// One part of the product: its health is `good`, `watch`, `poor` or `unknown`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Area {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub paths: Vec<String>,
    pub health: String,
    pub notes: String,
    /// The other areas it talks to, by id.
    pub links: Vec<String>,
    pub updated_ms: i64,
    pub by: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Signal {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub text: String,
    pub from: String,
    pub url: Option<String>,
    pub environment: Option<String>,
    pub at_ms: i64,
}

/// A change the watchers propose. `state` is `proposed`, `later`, `approved`, `declined` or `withdrawn`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Proposal {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub summary: String,
    /// The area it touches; else the first of its impact's areas.
    pub area: Option<String>,
    pub impact: Impact,
    /// 1 (little) to 5 (a lot), and why.
    pub value: Worth,
    pub effort: Effort,
    pub evidence: Vec<Evidence>,
    /// Data the watchers lack, and where it would come from.
    pub instrument: Option<Instrument>,
    pub role: String,
    pub acceptance: String,
    pub state: String,
    pub by: String,
    pub created_ms: i64,
    pub decided_ms: i64,
    pub note: String,
    /// The task it became, and that task's state.
    pub task: Option<String>,
    pub task_state: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Impact {
    pub changes: String,
    pub users: String,
    pub areas: Vec<String>,
    /// `low`, `medium` or `high`.
    pub risk: String,
    pub risk_note: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Worth {
    pub score: i64,
    pub why: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Effort {
    /// `s`, `m`, `l` or `xl`.
    pub size: String,
    pub budget: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Evidence {
    pub signal: Option<String>,
    pub note: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Instrument {
    pub kind: String,
    pub name: String,
    /// A connector or MCP preset the owner sets up, when there is one.
    pub catalog: Option<String>,
    pub data: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Sources {
    /// The inbox's count of signals.
    pub inbox: i64,
    /// Whether an ingest address is set.
    pub ingest: bool,
    pub connectors: Vec<Connector>,
    pub mcp: Vec<McpServer>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Connector {
    pub id: String,
    pub label: String,
    pub category: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct McpServer {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Sweep {
    pub runs: i64,
    pub last_ms: i64,
    pub last_summary: String,
    pub last_cid: u64,
    /// Set while a sweep runs.
    pub running: Option<Running>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Running {
    pub cid: u64,
}

/// `POST .../ingest`: the address's path, and the full address the tab builds from the hub's own address.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Ingest {
    pub path: String,
    #[serde(skip)]
    pub url: String,
}

/// `GET /v1/agents`, for the names of the watchers.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct AgentName {
    cid: u64,
    name: String,
}

// ---- The tab's state and messages ----------------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct State {
    /// The view; None until the first load.
    pub data: Option<Maintenance>,
    /// The names of the agents, by cid (a watcher without one shows as "agent N").
    pub names: BTreeMap<u64, String>,
    pub error: Option<String>,
    /// A line for the last action ("Approved.").
    pub note: Option<String>,
    /// A write is in flight.
    pub busy: bool,
    /// The focus as typed; None shows the saved one.
    pub focus_draft: Option<String>,
    /// What the owner chose on the map or in the list: shown beside the map.
    pub selection: Selection,
    /// Expert: every field, the decided proposals, the whole inbox.
    pub expert: bool,
    /// Whether "What the last sweep said" is open.
    pub summary_open: bool,
    /// The one dialog open, if any.
    pub dialog: Option<Dialog>,
    /// The signal being added (its fields are kept while the dialog is open).
    pub signal: NewSignal,
}

/// What the owner chose: nothing, one area of the map, or one proposal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Selection {
    #[default]
    None,
    Area(String),
    Proposal(String),
}

/// The dialog open over the tab. Only one is open at a time.
#[derive(Debug)]
pub enum Dialog {
    /// Do it: the note, and the budget for a proposal that has none of its own.
    Approve(Approving),
    /// Not this: the reason, optional.
    Decline(Declining),
    /// Add a signal (its fields are in `State::signal`).
    Signal,
    /// Sweeps: the interval the owner picks, in hours.
    Sweeps(i64),
    /// The ingest address, shown once: its full address and an example to post to it.
    Ingest { url: String, example: String },
}

#[derive(Debug)]
pub struct Approving {
    pub id: String,
    pub title: String,
    /// The line under the title: what the dialog says the answer does.
    pub line: String,
    /// The proposal names a connector or MCP the owner sets up, so it needs no budget.
    pub known_tool: bool,
    pub budget_hint: String,
    pub budget: String,
    pub note: Content,
}

#[derive(Debug)]
pub struct Declining {
    pub id: String,
    pub title: String,
    pub reason: Content,
}

#[derive(Debug)]
pub struct NewSignal {
    pub kind: String,
    pub title: String,
    pub text: Content,
    pub from: String,
}

impl Default for NewSignal {
    fn default() -> Self {
        NewSignal {
            kind: "feedback".into(),
            title: String::new(),
            text: Content::new(),
            from: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Msg {
    Go(Id),
    Loaded(Result<Maintenance, String>),
    Names(Result<Vec<(u64, String)>, String>),
    /// Sweep now.
    Sweep,
    Focus(String),
    SaveFocus,
    FocusSaved(Result<String, String>),
    OpenSweeps,
    PickEvery(i64),
    /// Sweeps every n hours (the dialog's Save).
    SaveEvery,
    /// Closes the dialog open over the tab.
    CloseDialog,
    Select(Selection),
    Expert(bool),
    ToggleSummary,
    OpenApprove(String),
    ApproveNote(text_editor::Action),
    ApproveBudget(String),
    Approve,
    Later(String),
    OpenDecline(String),
    DeclineReason(text_editor::Action),
    SubmitDecline,
    OpenSignal,
    SignalKind(String),
    SignalTitle(String),
    SignalText(text_editor::Action),
    SignalFrom(String),
    AddSignal,
    DeleteSignal(String),
    NewIngest,
    Ingest(Result<Ingest, String>),
    StopIngest,
    /// Puts the project in maintenance (the intro's button, and the note's).
    SetMode,
    /// A write went through (its line) or failed; the view is read again. A dialog closes on success.
    Done(Result<String, String>),
    /// Opens a link in the browser.
    OpenUrl(String),
}

/// The navigation this tab asks for.
pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

fn maintenance_path(project: &str) -> String {
    format!("/v1/projects/{}/maintenance", client::enc(project))
}

/// Reads the project's maintenance view, and the agents' names for its watchers.
pub fn load(project: &str, ctx: &Ctx) -> Task<Msg> {
    let c = ctx.client.clone();
    let path = maintenance_path(project);
    let names = ctx.client.clone();
    Task::batch([
        client::task(move || c.get::<Maintenance>(&path), Msg::Loaded),
        client::task(
            move || {
                names.get::<Vec<AgentName>>("/v1/agents").map(|list| {
                    list.into_iter()
                        .map(|a| (a.cid, a.name))
                        .collect::<Vec<(u64, String)>>()
                })
            },
            Msg::Names,
        ),
    ])
}

/// `project` is the project's id.
pub fn update(state: &mut State, msg: Msg, project: &str, ctx: &Ctx) -> Task<Msg> {
    let root = maintenance_path(project);
    match msg {
        Msg::Go(_) => Task::none(),
        Msg::Loaded(Ok(data)) => {
            state.data = Some(data);
            state.error = None;
            Task::none()
        }
        Msg::Loaded(Err(e)) => {
            state.error = Some(e);
            Task::none()
        }
        Msg::Names(Ok(list)) => {
            state.names = list.into_iter().collect();
            Task::none()
        }
        // The names are a courtesy: without them the watchers show by number.
        Msg::Names(Err(_)) => Task::none(),
        Msg::Sweep => write(
            state,
            ctx,
            move |c| {
                c.post::<Value>(&format!("{root}/sweep"), &json!({}))
                    .map(|_| "A watcher is sweeping.".to_string())
            },
            Msg::Done,
        ),
        Msg::Focus(typed) => {
            state.focus_draft = Some(typed.chars().take(FOCUS_MAX).collect());
            Task::none()
        }
        Msg::SaveFocus => {
            let Some(focus) = state.focus_draft.clone() else {
                return Task::none();
            };
            write(
                state,
                ctx,
                move |c| {
                    c.patch::<Value>(&root, &json!({ "focus": focus }))
                        .map(|_| "The watchers will read it.".to_string())
                },
                Msg::FocusSaved,
            )
        }
        Msg::FocusSaved(res) => {
            state.busy = false;
            if res.is_ok() {
                state.focus_draft = None;
            }
            settle(state, res);
            load(project, ctx)
        }
        Msg::OpenSweeps => {
            if let Some(data) = &state.data {
                state.dialog = Some(Dialog::Sweeps(data.config.every_hours));
            }
            Task::none()
        }
        Msg::PickEvery(hours) => {
            state.dialog = Some(Dialog::Sweeps(hours));
            Task::none()
        }
        Msg::SaveEvery => {
            let Some(Dialog::Sweeps(hours)) = state.dialog.as_ref() else {
                return Task::none();
            };
            let hours = *hours;
            write(
                state,
                ctx,
                move |c| {
                    c.patch::<Value>(&root, &json!({ "every_hours": hours }))
                        .map(|_| "Saved.".to_string())
                },
                Msg::Done,
            )
        }
        Msg::CloseDialog => {
            state.dialog = None;
            state.error = None;
            Task::none()
        }
        Msg::Select(sel) => {
            state.selection = sel;
            Task::none()
        }
        Msg::Expert(on) => {
            state.expert = on;
            Task::none()
        }
        Msg::ToggleSummary => {
            state.summary_open = !state.summary_open;
            Task::none()
        }
        Msg::OpenApprove(pid) => {
            let Some(p) = find(state, &pid) else {
                return Task::none();
            };
            let known_tool = p.instrument.as_ref().is_some_and(|i| i.catalog.is_some());
            let line = match &p.instrument {
                Some(i) if known_tool => format!(
                    "{} is something you set up: it is picked, and you add it in Connectors or MCP.",
                    i.name
                ),
                _ => format!(
                    "It becomes a task of this project for a {}. {}.",
                    p.role,
                    weigh_line(p)
                ),
            };
            state.dialog = Some(Dialog::Approve(Approving {
                id: p.id.clone(),
                title: p.title.clone(),
                line,
                known_tool,
                budget_hint: p
                    .effort
                    .budget
                    .map_or("The project’s default".to_string(), |b| b.to_string()),
                budget: String::new(),
                note: Content::new(),
            }));
            Task::none()
        }
        Msg::ApproveNote(action) => {
            if let Some(Dialog::Approve(a)) = state.dialog.as_mut() {
                a.note.perform(action);
            }
            Task::none()
        }
        Msg::ApproveBudget(budget) => {
            if let Some(Dialog::Approve(a)) = state.dialog.as_mut() {
                a.budget = budget;
            }
            Task::none()
        }
        Msg::Approve => {
            let Some(Dialog::Approve(a)) = state.dialog.as_ref() else {
                return Task::none();
            };
            let mut body = json!({});
            let note = a.note.text().trim().to_string();
            if !note.is_empty() {
                body["note"] = json!(note);
            }
            // A budget is credits, at least 200; under that, the project's default stands.
            if let Some(credits) = a.budget.trim().parse::<i64>().ok().filter(|c| *c >= 200) {
                body["budget"] = json!(credits);
            }
            let path = format!("{root}/proposals/{}/approve", a.id);
            write(
                state,
                ctx,
                move |c| {
                    c.post::<Value>(&path, &body)
                        .map(|_| "Picked: it is a task now.".to_string())
                },
                Msg::Done,
            )
        }
        Msg::Later(pid) => write(
            state,
            ctx,
            move |c| {
                c.post::<Value>(&format!("{root}/proposals/{pid}/later"), &json!({}))
                    .map(|_| "Put off for later.".to_string())
            },
            Msg::Done,
        ),
        Msg::OpenDecline(pid) => {
            let Some(p) = find(state, &pid) else {
                return Task::none();
            };
            state.dialog = Some(Dialog::Decline(Declining {
                id: p.id.clone(),
                title: p.title.clone(),
                reason: Content::new(),
            }));
            Task::none()
        }
        Msg::DeclineReason(action) => {
            if let Some(Dialog::Decline(d)) = state.dialog.as_mut() {
                d.reason.perform(action);
            }
            Task::none()
        }
        Msg::SubmitDecline => {
            let Some(Dialog::Decline(d)) = state.dialog.as_ref() else {
                return Task::none();
            };
            let reason = d.reason.text().trim().to_string();
            let path = format!("{root}/proposals/{}/decline", d.id);
            write(
                state,
                ctx,
                move |c| {
                    c.post::<Value>(&path, &json!({ "reason": reason }))
                        .map(|_| "Declined.".to_string())
                },
                Msg::Done,
            )
        }
        Msg::OpenSignal => {
            state.signal = NewSignal::default();
            state.dialog = Some(Dialog::Signal);
            Task::none()
        }
        Msg::SignalKind(kind) => {
            state.signal.kind = kind;
            Task::none()
        }
        Msg::SignalTitle(title) => {
            state.signal.title = title;
            Task::none()
        }
        Msg::SignalText(action) => {
            state.signal.text.perform(action);
            Task::none()
        }
        Msg::SignalFrom(from) => {
            state.signal.from = from;
            Task::none()
        }
        Msg::AddSignal => {
            let text = state.signal.text.text().trim().to_string();
            if text.is_empty() {
                state.error = Some("Say what the signal says.".into());
                return Task::none();
            }
            let body = json!({
                "kind": state.signal.kind,
                "title": state.signal.title.trim(),
                "text": text,
                "from": state.signal.from.trim(),
            });
            write(
                state,
                ctx,
                move |c| {
                    c.post::<Value>(&format!("{root}/signals"), &body)
                        .map(|_| "Signal added.".to_string())
                },
                Msg::Done,
            )
        }
        Msg::DeleteSignal(sid) => write(
            state,
            ctx,
            move |c| {
                c.delete(&format!("{root}/signals/{sid}"))
                    .map(|_| "Signal deleted.".to_string())
            },
            Msg::Done,
        ),
        Msg::NewIngest => {
            if !start(state) {
                return Task::none();
            }
            let c = ctx.client.clone();
            let base = c.base().to_string();
            let path = format!("{root}/ingest");
            client::task(
                move || {
                    c.post::<Ingest>(&path, &json!({})).map(|mut i| {
                        i.url = format!("{base}{}", i.path);
                        i
                    })
                },
                Msg::Ingest,
            )
        }
        Msg::Ingest(res) => {
            state.busy = false;
            match res {
                Ok(ingest) => {
                    let example = format!(
                        "curl -X POST {url} \\\n  -H 'content-type: application/json' \\\n  -d '{{\"kind\": \"report\", \"title\": \"Checkout fails\", \"text\": \"The pay button does nothing on Safari\"}}'",
                        url = ingest.url
                    );
                    state.dialog = Some(Dialog::Ingest {
                        url: ingest.url,
                        example,
                    });
                    state.error = None;
                }
                Err(e) => state.error = Some(e),
            }
            load(project, ctx)
        }
        Msg::StopIngest => write(
            state,
            ctx,
            move |c| {
                c.delete(&format!("{root}/ingest"))
                    .map(|_| "The ingest address stopped.".to_string())
            },
            Msg::Done,
        ),
        Msg::SetMode => {
            let path = format!("/v1/projects/{}/mode", client::enc(project));
            write(
                state,
                ctx,
                move |c| {
                    c.post::<Value>(&path, &json!({ "mode": "maintenance" }))
                        .map(|_| "The project is in maintenance.".to_string())
                },
                Msg::Done,
            )
        }
        Msg::Done(res) => {
            state.busy = false;
            if res.is_ok() {
                state.dialog = None;
            }
            settle(state, res);
            load(project, ctx)
        }
        Msg::OpenUrl(url) => {
            if let Err(e) = ui::open_url(&url) {
                state.error = Some(e);
            }
            Task::none()
        }
    }
}

/// The proposal with this id, if the view has it.
fn find<'s>(state: &'s State, pid: &str) -> Option<&'s Proposal> {
    state.data.as_ref()?.proposals.iter().find(|p| p.id == pid)
}

/// Starts a write, unless one is under way: a click while it runs sends nothing.
fn start(state: &mut State) -> bool {
    if state.busy {
        return false;
    }
    state.busy = true;
    true
}

/// Runs one write as a task, busy until it answers; `done` gets its line or its error.
fn write(
    state: &mut State,
    ctx: &Ctx,
    job: impl FnOnce(client::Client) -> Result<String, String> + Send + 'static,
    done: fn(Result<String, String>) -> Msg,
) -> Task<Msg> {
    if !start(state) {
        return Task::none();
    }
    let c = ctx.client.clone();
    client::task(move || job(c), done)
}

/// Shows a write's line, or its error.
fn settle(state: &mut State, res: Result<String, String>) {
    match res {
        Ok(line) => {
            state.note = Some(line);
            state.error = None;
        }
        Err(e) => state.error = Some(e),
    }
}

// ---- Plain helpers (tested below) ----------------------------------------------------------------------------------

/// A proposal the owner has still to decide: proposed, or later.
fn is_waiting(p: &Proposal) -> bool {
    matches!(p.state.as_str(), "proposed" | "later")
}

/// How many proposals wait for the owner.
pub fn waiting_count(props: &[Proposal]) -> usize {
    props.iter().filter(|p| is_waiting(p)).count()
}

/// The proposals waiting for the owner, best first: value high to low, then risk low to high, then the ones not put
/// off, then the newest.
pub fn waiting_in_order(props: &[Proposal]) -> Vec<&Proposal> {
    let mut open: Vec<&Proposal> = props.iter().filter(|p| is_waiting(p)).collect();
    open.sort_by(|a, b| {
        b.value
            .score
            .cmp(&a.value.score)
            .then_with(|| risk_rank(&a.impact.risk).cmp(&risk_rank(&b.impact.risk)))
            .then_with(|| (a.state == "later").cmp(&(b.state == "later")))
            .then_with(|| b.created_ms.cmp(&a.created_ms))
    });
    open
}

/// The proposals already decided (approved, declined, withdrawn), newest decision first.
pub fn decided_of(props: &[Proposal]) -> Vec<&Proposal> {
    let mut done: Vec<&Proposal> = props.iter().filter(|p| !is_waiting(p)).collect();
    done.sort_by_key(|p| std::cmp::Reverse(p.decided_ms));
    done
}

fn risk_rank(risk: &str) -> u8 {
    match risk {
        "low" => 0,
        "medium" => 1,
        "high" => 2,
        _ => 3,
    }
}

/// A health's dot: good is ok, watch is warn, poor is danger, anything else muted.
pub fn health_tone(health: &str) -> ui::Tone {
    match health {
        "good" => ui::Tone::Ok,
        "watch" => ui::Tone::Warn,
        "poor" => ui::Tone::Danger,
        _ => ui::Tone::Muted,
    }
}

/// A risk's words' colour: low is ok, medium warn, high danger.
fn risk_tone(risk: &str) -> ui::Tone {
    match risk {
        "low" => ui::Tone::Ok,
        "medium" => ui::Tone::Warn,
        "high" => ui::Tone::Danger,
        _ => ui::Tone::Muted,
    }
}

/// A proposal kind's badge: security is danger, a data proposal info, the rest muted.
fn kind_tone(kind: &str) -> ui::Tone {
    match kind {
        "security" => ui::Tone::Danger,
        "instrument" => ui::Tone::Info,
        _ => ui::Tone::Muted,
    }
}

/// A proposal kind in words.
fn kind_label(kind: &str) -> String {
    match kind {
        "fix" => "Fix",
        "improve" => "Improve",
        "feature" => "Feature",
        "security" => "Security",
        "performance" => "Performance",
        "chore" => "Chore",
        "instrument" => "More data",
        other => other,
    }
    .to_string()
}

/// A proposal's size, short: S, M, L or XL.
fn size_short(size: &str) -> String {
    size.to_uppercase()
}

/// A proposal's size in words.
fn size_label(size: &str) -> &str {
    match size {
        "s" => "small",
        "m" => "medium",
        "l" => "large",
        "xl" => "very large",
        other => other,
    }
}

/// "Value 4/5 · low risk · small".
fn weigh_line(p: &Proposal) -> String {
    format!(
        "Value {}/5 · {} risk · {}",
        p.value.score,
        p.impact.risk,
        size_label(&p.effort.size)
    )
}

/// What became of a decided proposal, in words.
fn outcome_line(p: &Proposal) -> String {
    match p.state.as_str() {
        "approved" => match (&p.task, &p.instrument) {
            (Some(task), _) if !task.is_empty() => {
                match p.task_state.as_deref().filter(|s| !s.is_empty()) {
                    Some(ts) => format!("Picked: task {task} ({})", ts.replace('_', " ")),
                    None => format!("Picked: task {task}"),
                }
            }
            (_, Some(i)) if i.catalog.is_some() => {
                format!("Picked: set up {} in Connectors or MCP", i.name)
            }
            _ => "Picked".into(),
        },
        "declined" => {
            if p.note.is_empty() {
                "Declined".into()
            } else {
                format!("Declined: {}", p.note)
            }
        }
        "withdrawn" => {
            if p.note.is_empty() {
                "Withdrawn".into()
            } else {
                format!("Withdrawn: {}", p.note)
            }
        }
        other => other.to_string(),
    }
}

/// The area a proposal touches: its own, else the first of its impact's.
fn area_of(p: &Proposal) -> Option<&str> {
    p.area
        .as_deref()
        .filter(|a| !a.is_empty())
        .or_else(|| p.impact.areas.first().map(String::as_str))
        .filter(|a| !a.is_empty())
}

/// The waiting proposals of each area, best first.
fn waiting_by_area(props: &[Proposal]) -> BTreeMap<String, Vec<&Proposal>> {
    let mut out: BTreeMap<String, Vec<&Proposal>> = BTreeMap::new();
    for p in waiting_in_order(props) {
        if let Some(area) = area_of(p) {
            out.entry(area.to_string()).or_default().push(p);
        }
    }
    out
}

/// The headline of the tab: what waits for the owner.
fn headline(waiting: usize, runs: i64, running: bool) -> String {
    match waiting {
        0 if runs == 0 && !running => "No sweep yet".into(),
        0 => "Nothing waits for you".into(),
        1 => "One proposal waits for you".into(),
        n => format!("{n} proposals wait for you"),
    }
}

/// "every 24 h", "only when you ask".
fn every_text(hours: i64) -> String {
    match hours {
        0 => "only when you ask".into(),
        1 => "every hour".into(),
        h => format!("every {h} h"),
    }
}

/// The Sweeps dialog's choice, labelled as the web does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Every(i64);

impl fmt::Display for Every {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&every_text(self.0))
    }
}

/// A signal in one line: its title, else the start of its text (at most 120 characters).
fn signal_line(s: &Signal) -> String {
    let title = s.title.trim();
    let line = if title.is_empty() {
        one_line(&s.text)
    } else {
        title.to_string()
    };
    if line.chars().count() > 120 {
        format!("{}…", line.chars().take(119).collect::<String>())
    } else {
        line
    }
}

/// Text on one line, its spaces collapsed.
fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// An evidence line: the signal it cites, then the note.
fn evidence_text(e: &Evidence) -> String {
    match (e.signal.as_deref().filter(|s| !s.is_empty()), e.note.trim()) {
        (Some(signal), "") => signal.to_string(),
        (Some(signal), note) => format!("{signal} · {note}"),
        (None, note) => note.to_string(),
    }
}

/// The sweep in a line: under way, or when it last ran and when the next is due, then the interval.
fn sweep_line(data: &Maintenance, names: &BTreeMap<u64, String>, now: i64) -> String {
    let name = |cid: u64| {
        names
            .get(&cid)
            .cloned()
            .unwrap_or_else(|| format!("agent {cid}"))
    };
    let s = &data.sweep;
    let every = every_text(data.config.every_hours);
    if let Some(r) = &s.running {
        return format!("{} is sweeping now", name(r.cid));
    }
    if s.runs == 0 {
        let line = if data.watchers.is_empty() {
            "Add an agent to the project to watch it"
        } else {
            "The first sweep maps the project"
        };
        return format!("{line} · {every}");
    }
    let ago = (now - s.last_ms).max(0);
    let ago_text = if ago < HOUR_MS {
        format!(
            "{} min ago",
            ((ago as f64) / 60_000.0).round().max(1.0) as i64
        )
    } else {
        format!("{} h ago", ((ago as f64) / HOUR_MS as f64).round() as i64)
    };
    let mut next = String::new();
    if data.config.every_hours > 0 {
        let left = s.last_ms + data.config.every_hours * HOUR_MS - now;
        next = if left <= 0 {
            " · the next is due".into()
        } else if left < HOUR_MS {
            format!(
                " · the next in {} min",
                ((left as f64) / 60_000.0).round().max(1.0) as i64
            )
        } else {
            format!(
                " · the next in {} h",
                ((left as f64) / HOUR_MS as f64).round() as i64
            )
        };
    }
    format!(
        "Last sweep {ago_text} by {}{next} · {every}",
        name(s.last_cid)
    )
}

const HOUR_MS: i64 = 3_600_000;

/// The current time, in milliseconds since the epoch.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// How long ago an instant was, in the words the web uses ("just now", "5 min ago", "3 h ago", "2 d ago").
fn ago(ms: i64, now: i64) -> String {
    if ms <= 0 {
        return "—".into();
    }
    let s = ((now - ms).max(0) as f64 / 1000.0).round() as i64;
    match s {
        0..=44 => "just now".into(),
        45..=3599 => format!("{} min ago", ((s as f64) / 60.0).round().max(1.0) as i64),
        3600..=86399 => format!("{} h ago", ((s as f64) / 3600.0).round() as i64),
        _ => format!("{} d ago", ((s as f64) / 86400.0).round() as i64),
    }
}

/// The map's ring: the areas evenly around the product, starting at the top.
fn ring(n: usize) -> Vec<Point> {
    let c = MAP / 2.0;
    let r = match n {
        0 | 1 => 0.0,
        n if n > 10 => MAP * 0.38,
        _ => MAP * 0.33,
    };
    (0..n)
        .map(|i| {
            let a = -PI / 2.0 + 2.0 * PI * i as f32 / n.max(1) as f32;
            Point::new(c + r * a.cos(), c + r * a.sin())
        })
        .collect()
}

/// The map's links as index pairs, each once.
fn map_links(areas: &[Area]) -> Vec<(usize, usize)> {
    let index: BTreeMap<&str, usize> = areas
        .iter()
        .enumerate()
        .map(|(i, a)| (a.id.as_str(), i))
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for (i, a) in areas.iter().enumerate() {
        for b in &a.links {
            let Some(&j) = index.get(b.as_str()) else {
                continue;
            };
            if j == i {
                continue;
            }
            let key = (i.min(j), i.max(j));
            if seen.insert(key) {
                out.push((i, j));
            }
        }
    }
    out
}

/// A waiting proposal's dot colour on the map: security is danger, a data proposal info, the rest ink.
fn dot_color(kind: &str) -> Color {
    let p = theme::palette();
    match kind {
        "security" => p.crit,
        "instrument" => p.info,
        _ => p.ink_2,
    }
}

// ---- The view ------------------------------------------------------------------------------------------------------

/// The tab. `in_maintenance` is whether the project runs in maintenance (the mode switch is on its page).
pub fn view<'a>(state: &'a State, _project: &'a str, in_maintenance: bool) -> Element<'a, Msg> {
    let mut parts: Vec<Element<'a, Msg>> = Vec::new();
    if let (None, Some(e)) = (&state.dialog, &state.error) {
        parts.push(ui::error_line(e));
    }
    if let Some(n) = &state.note {
        parts.push(ui::faint(n.clone()).into());
    }
    let Some(data) = &state.data else {
        parts.push(ui::loading());
        return with_dialog(state, column(parts).spacing(16).into());
    };
    if !in_maintenance
        && data.sweep.runs == 0
        && data.sweep.running.is_none()
        && data.proposals.is_empty()
        && data.signals.is_empty()
    {
        parts.push(intro());
        return with_dialog(state, column(parts).spacing(16).into());
    }
    if !in_maintenance {
        parts.push(
            container(
                row![
                    ui::faint("This project is in build mode now: its watchers no longer sweep."),
                    ui::btn_sm(
                        "Back to maintenance",
                        None,
                        ui::Kind::Ghost,
                        Some(Msg::SetMode)
                    ),
                ]
                .spacing(12)
                .align_y(Alignment::Center)
                .wrap(),
            )
            .padding(12)
            .width(Length::Fill)
            .style(ui::frame(
                Some(theme::palette().sheet_2),
                theme::palette().sheet_2,
            ))
            .into(),
        );
    }
    let names = &state.names;
    parts.push(strip(state, data, names));
    parts.push(focus_row(state, data));
    parts.push(stage(state, data));
    if !data.sweep.last_summary.is_empty() && data.sweep.runs > 0 {
        parts.push(last_sweep(state, data));
    }
    parts.push(proposals_card(state, data));
    let instruments: Vec<&Proposal> = waiting_in_order(&data.proposals)
        .into_iter()
        .filter(|p| p.instrument.is_some())
        .collect();
    if !instruments.is_empty() {
        parts.push(lacking_card(&instruments));
    }
    let done = decided_of(&data.proposals);
    if state.expert && !done.is_empty() {
        let items: Vec<Element<'a, Msg>> = done
            .iter()
            .map(|p| proposal_box(state, data, p, true))
            .collect();
        parts.push(ui::card(
            &format!("Decided ({})", done.len()),
            column(items).spacing(12).into(),
        ));
    } else if !done.is_empty() {
        parts.push(
            row![
                ui::faint(format!("{} decided — ", done.len())),
                link_button("show them", Msg::Expert(true)),
            ]
            .align_y(Alignment::Center)
            .into(),
        );
    }
    parts.push(inbox_card(state, data));
    parts.push(reads_card(data));
    with_dialog(state, column(parts).spacing(16).into())
}

/// The intro for a project that is not in maintenance and has nothing yet: what maintenance is, and the button.
fn intro<'a>() -> Element<'a, Msg> {
    column![
        ui::empty::<Msg>("search", BUILDING_TITLE, BUILDING),
        row![ui::btn(
            "Put it in maintenance",
            None,
            ui::Kind::Primary,
            Some(Msg::SetMode)
        )]
        .align_y(Alignment::Center),
    ]
    .spacing(12)
    .align_x(Alignment::Center)
    .width(Length::Fill)
    .into()
}

/// The dialog open over the tab, if one is, laid over the tab's content.
fn with_dialog<'a>(state: &'a State, base: Element<'a, Msg>) -> Element<'a, Msg> {
    let dialog = state
        .data
        .as_ref()
        .and_then(|data| dialog_view(state, data));
    ui::with_modal(base, dialog)
}

/// The dialog's frame: the title, the body with any error above it, and the buttons in the foot.
fn modal_of<'a>(
    state: &'a State,
    title: &'static str,
    mut body: Vec<Element<'a, Msg>>,
    foot: Vec<Element<'a, Msg>>,
) -> Element<'a, Msg> {
    if let Some(e) = &state.error {
        body.insert(0, ui::error_line(e));
    }
    ui::modal(
        title,
        column(body).spacing(14).into(),
        foot,
        Msg::CloseDialog,
        480.0,
    )
}

/// A dialog's field: its label, the control, and its hint under it.
fn field<'a>(
    label: &str,
    control: Element<'a, Msg>,
    hint: Option<&'static str>,
) -> Element<'a, Msg> {
    let mut lines: Vec<Element<'a, Msg>> = vec![ui::label(label).into(), control];
    if let Some(h) = hint {
        lines.push(ui::faint(h).into());
    }
    column(lines).spacing(6).into()
}

/// The dialog open, drawn: Do it, Not this, Add a signal, Sweeps, or the ingest address.
fn dialog_view<'a>(state: &'a State, data: &'a Maintenance) -> Option<Element<'a, Msg>> {
    let pal = theme::palette();
    Some(match state.dialog.as_ref()? {
        Dialog::Approve(a) => {
            let mut body: Vec<Element<'a, Msg>> = vec![
                text(a.title.clone()).font(ui::SANS_SEMIBOLD).size(theme::TYPE_ITEM).color(pal.ink).into(),
                ui::faint(a.line.clone()).into(),
            ];
            if !a.known_tool {
                body.push(field(
                    "Budget (credits)",
                    text_input(&a.budget_hint, &a.budget)
                        .on_input(Msg::ApproveBudget)
                        .padding(8)
                        .style(theme::input)
                        .into(),
                    None,
                ));
            }
            body.push(field(
                "Note",
                text_editor(&a.note)
                    .style(crate::theme::editor)
                    .placeholder("Anything the team should know (optional)")
                    .size(13.0)
                    .height(Length::Fixed(60.0))
                    .on_action(Msg::ApproveNote)
                    .into(),
                None,
            ));
            modal_of(
                state,
                "Do it",
                body,
                vec![
                    ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CloseDialog)),
                    ui::btn("Do it", None, ui::Kind::Primary, (!state.busy).then_some(Msg::Approve)),
                ],
            )
        }
        Dialog::Decline(d) => modal_of(
            state,
            "Not this",
            vec![
                text(d.title.clone()).font(ui::SANS_SEMIBOLD).size(theme::TYPE_ITEM).color(pal.ink).into(),
                field(
                    "Reason (optional)",
                    text_editor(&d.reason)
                        .style(crate::theme::editor)
                        .placeholder("Why not: the watchers read it, and propose it again only with new evidence")
                        .size(13.0)
                        .height(Length::Fixed(80.0))
                        .on_action(Msg::DeclineReason)
                        .into(),
                    None,
                ),
            ],
            vec![
                ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CloseDialog)),
                ui::btn("Decline", None, ui::Kind::Danger, (!state.busy).then_some(Msg::SubmitDecline)),
            ],
        ),
        Dialog::Signal => {
            let ns = &state.signal;
            let kinds: Vec<String> = SIGNAL_KINDS.iter().map(|k| k.to_string()).collect();
            modal_of(
                state,
                "Add a signal",
                vec![
                    field(
                        "Kind",
                        pick_list(kinds, Some(ns.kind.clone()), Msg::SignalKind)
                            .style(theme::pick_list)
                            .text_size(13.0)
                            .into(),
                        None,
                    ),
                    field(
                        "Title (optional)",
                        text_input("Checkout fails on Safari", &ns.title)
                            .on_input(Msg::SignalTitle)
                            .padding(8)
                            .style(theme::input)
                            .into(),
                        None,
                    ),
                    field(
                        "What it says",
                        text_editor(&ns.text)
                            .style(crate::theme::editor)
                            .placeholder("What a user said, a figure from your analytics, an error report…")
                            .size(13.0)
                            .height(Length::Fixed(96.0))
                            .on_action(Msg::SignalText)
                            .into(),
                        None,
                    ),
                    field(
                        "From (optional)",
                        text_input("support, a customer, your analytics", &ns.from)
                            .on_input(Msg::SignalFrom)
                            .padding(8)
                            .style(theme::input)
                            .into(),
                        None,
                    ),
                ],
                vec![
                    ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CloseDialog)),
                    ui::btn("Add", None, ui::Kind::Primary, (!state.busy).then_some(Msg::AddSignal)),
                ],
            )
        }
        Dialog::Sweeps(hours) => {
            let choices: Vec<Every> = EVERY_CHOICES.iter().map(|&h| Every(h)).collect();
            let watchers = if data.watchers.is_empty() {
                "No agent watches it yet: add a planner or a researcher on the Team tab.".to_string()
            } else {
                let names: Vec<String> = data
                    .watchers
                    .iter()
                    .map(|cid| state.names.get(cid).cloned().unwrap_or_else(|| format!("agent {cid}")))
                    .collect();
                format!("Watchers, in turn: {}.", names.join(", "))
            };
            modal_of(
                state,
                "Sweeps",
                vec![
                    field(
                        "Sweep",
                        pick_list(choices, Some(Every(*hours)), |e: Every| Msg::PickEvery(e.0))
                            .style(theme::pick_list)
                            .text_size(13.0)
                            .into(),
                        Some("A sweep is one watcher reading what came in, updating the map and proposing."),
                    ),
                    ui::faint(watchers).into(),
                ],
                vec![
                    ui::btn("Cancel", None, ui::Kind::Ghost, Some(Msg::CloseDialog)),
                    ui::btn("Save", None, ui::Kind::Primary, (!state.busy).then_some(Msg::SaveEvery)),
                ],
            )
        }
        Dialog::Ingest { url, example } => modal_of(
            state,
            "The ingest address",
            vec![
                ui::faint("Send user reports, feedback and webhooks here. It is shown once: copy it now. Making a new one stops this one.").into(),
                pre(url.clone()),
                ui::faint("JSON with text (and kind, title, from, url, environment), or plain text:").into(),
                pre(example.clone()),
            ],
            Vec::new(),
        ),
    })
}

/// A block of mono text on the sheet's second ground: the web's `pre`.
fn pre<'a>(content: String) -> Element<'a, Msg> {
    let pal = theme::palette();
    container(ui::mono(content, theme::TYPE_CODE, pal.ink))
        .padding(12)
        .width(Length::Fill)
        .style(ui::frame(Some(pal.sheet_2), pal.sheet_2))
        .into()
}

/// A text button in the link's colour (signal ink), as the web's `.mt-linkbtn`.
fn link_button<'a>(label: &str, msg: Msg) -> Element<'a, Msg> {
    let ink = theme::palette().signal_ink;
    button(
        text(label.to_string())
            .font(ui::SANS_MEDIUM)
            .size(theme::TYPE_UI)
            .color(ink),
    )
    .padding(0.0)
    .style(|_: &Theme, _: button::Status| button::Style {
        background: None,
        text_color: theme::palette().signal_ink,
        border: Border::default(),
        ..Default::default()
    })
    .on_press(msg)
    .into()
}

/// A small disc in a colour (the health dots, the value dots).
fn disc<'a>(color: Color, size: f32) -> Element<'a, Msg> {
    container(
        Space::new()
            .width(Length::Fixed(size))
            .height(Length::Fixed(size)),
    )
    .style(move |_: &Theme| container::Style {
        background: Some(color.into()),
        border: Border::default().rounded(size / 2.0),
        ..Default::default()
    })
    .into()
}

/// The value of a proposal as five dots, filled up to its score.
fn value_dots<'a>(score: i64) -> Element<'a, Msg> {
    let pal = theme::palette();
    row((1..=5).map(|i| {
        disc(
            if i <= score {
                pal.signal
            } else {
                pal.rule_strong
            },
            6.0,
        )
    }))
    .spacing(3)
    .align_y(Alignment::Center)
    .into()
}

/// A labelled value in the web's `.mt-k`: the label small and strong, the value under it.
fn kfield<'a>(label: &str, value: String) -> Element<'a, Msg> {
    let pal = theme::palette();
    column![
        text(label.to_string())
            .font(ui::SANS_SEMIBOLD)
            .size(theme::TYPE_BADGE)
            .color(pal.ink_3),
        ui::sans(value, theme::TYPE_UI, pal.ink),
    ]
    .spacing(4)
    .into()
}

/// The strip: the headline, the sweep line; Sweep now, the sweeps' settings, and Expert.
fn strip<'a>(
    state: &'a State,
    data: &'a Maintenance,
    names: &BTreeMap<u64, String>,
) -> Element<'a, Msg> {
    let pal = theme::palette();
    let waiting = waiting_count(&data.proposals);
    let running = data.sweep.running.is_some();
    let mut head: Vec<Element<'a, Msg>> = Vec::new();
    if waiting > 0 {
        head.push(disc(pal.signal, 8.0));
    }
    head.push(
        text(headline(waiting, data.sweep.runs, running))
            .font(ui::SANS_SEMIBOLD)
            .size(theme::TYPE_ITEM)
            .color(pal.ink)
            .into(),
    );
    let titles = column![
        row(head).spacing(8).align_y(Alignment::Center),
        ui::faint(sweep_line(data, names, now_ms())).size(theme::TYPE_META),
    ]
    .spacing(4);
    let acts = row![
        ui::btn_sm(
            "Sweep now",
            None,
            ui::Kind::Secondary,
            (!state.busy && !running).then_some(Msg::Sweep),
        ),
        ui::btn_sm("", Some("settings"), ui::Kind::Ghost, Some(Msg::OpenSweeps)),
        checkbox(state.expert)
            .label("Expert")
            .on_toggle(Msg::Expert)
            .size(14.0)
            .text_size(13.0),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .wrap();
    row![titles.width(Length::Fill), acts]
        .spacing(16)
        .align_y(Alignment::Start)
        .wrap()
        .into()
}

/// The focus: what the watchers should look at first. It is saved on Enter.
fn focus_row<'a>(state: &'a State, data: &'a Maintenance) -> Element<'a, Msg> {
    let draft = state
        .focus_draft
        .as_deref()
        .unwrap_or(data.config.focus.as_str());
    row![
        ui::label("Focus").width(Length::Fixed(64.0)),
        text_input(
            "What matters now: checkout conversion, the mobile crashes…",
            draft
        )
        .on_input(Msg::Focus)
        .on_submit(Msg::SaveFocus)
        .padding(8)
        .style(theme::input)
        .width(Length::Fill),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into()
}

/// The stage: the map of the product, and beside it the side panel.
fn stage<'a>(state: &'a State, data: &'a Maintenance) -> Element<'a, Msg> {
    row![
        container(map_body(state, data))
            .padding(16)
            .width(Length::Fill)
            .style(ui::frame(
                Some(theme::palette().sheet),
                theme::palette().rule
            )),
        container(side_panel(state, data))
            .padding(16)
            .width(Length::Fixed(SIDE))
            .style(ui::frame(
                Some(theme::palette().sheet),
                theme::palette().rule
            )),
    ]
    .spacing(16)
    .into()
}

/// The map: the graph of the product, or the line that says there is none yet.
fn map_body<'a>(state: &'a State, data: &'a Maintenance) -> Element<'a, Msg> {
    let pal = theme::palette();
    let areas = &data.map.areas;
    if areas.is_empty() {
        let line = if data.sweep.running.is_some() {
            "The watchers are mapping the project…"
        } else {
            "No map yet: the first sweep maps the project."
        };
        return container(ui::faint(line))
            .padding(12)
            .width(Length::Fill)
            .style(ui::frame(Some(pal.sheet_2), pal.sheet_2))
            .into();
    }
    let at = ring(areas.len());
    let by_area = waiting_by_area(&data.proposals);
    let selected = match &state.selection {
        Selection::Area(id) => Some(id.as_str()),
        _ => None,
    };
    let dots: Vec<MapArea> = areas
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let waiting = by_area.get(&a.id).map(|v| v.as_slice()).unwrap_or_default();
            MapArea {
                id: a.id.clone(),
                at: at[i],
                color: health_tone(&a.health).color(),
                on: selected == Some(a.id.as_str()),
                label: {
                    let n = a.name.chars().count();
                    if n > 18 {
                        format!("{}…", a.name.chars().take(17).collect::<String>())
                    } else {
                        a.name.clone()
                    }
                },
                waiting: waiting
                    .iter()
                    .take(DOTS_PER_AREA)
                    .map(|p| dot_color(&p.kind))
                    .collect(),
            }
        })
        .collect();
    let links = map_links(areas)
        .into_iter()
        .map(|(a, b)| (at[a], at[b]))
        .collect();
    // The map is drawn at its own size and centred in its card, as the web's svg is.
    let graph = Canvas::new(MapView {
        heard: data.unseen > 0,
        links,
        areas: dots,
    })
    .width(Length::Fixed(MAP))
    .height(Length::Fixed(MAP));
    container(graph).center_x(Length::Fill).into()
}

/// One area on the map, in the map's own square.
struct MapArea {
    id: String,
    at: Point,
    color: Color,
    on: bool,
    label: String,
    /// The colours of its waiting proposals, one dot each.
    waiting: Vec<Color>,
}

/// The map as the canvas draws it: spokes from the product to each area, the areas' own links dashed, the areas,
/// their waiting proposals as dots around them, and their names under them. A click on an area chooses it (again to
/// let it go); a click on the background clears the choice.
struct MapView {
    heard: bool,
    links: Vec<(Point, Point)>,
    areas: Vec<MapArea>,
}

impl MapView {
    fn centre() -> Point {
        Point::new(MAP / 2.0, MAP / 2.0)
    }

    /// The area under a point of the map, if any.
    fn area_at(&self, at: Point) -> Option<&MapArea> {
        self.areas.iter().find(|a| a.at.distance(at) <= 15.0)
    }
}

impl canvas::Program<Msg> for MapView {
    type State = ();

    fn update(
        &self,
        _state: &mut (),
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Msg>> {
        let canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event else {
            return None;
        };
        let at = cursor.position_in(bounds)?;
        let msg = match self.area_at(at) {
            Some(a) if a.on => Msg::Select(Selection::None),
            Some(a) => Msg::Select(Selection::Area(a.id.clone())),
            None => Msg::Select(Selection::None),
        };
        Some(canvas::Action::publish(msg).and_capture())
    }

    fn mouse_interaction(
        &self,
        _state: &(),
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        match cursor.position_in(bounds) {
            Some(at) if self.area_at(at).is_some() => mouse::Interaction::Pointer,
            _ => mouse::Interaction::default(),
        }
    }

    fn draw(
        &self,
        _state: &(),
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry<Renderer>> {
        let pal = theme::palette();
        let mut frame = Frame::new(renderer, bounds.size());
        let c = Self::centre();
        let spoke = Stroke::default()
            .with_color(pal.rule_strong)
            .with_width(1.0);
        for a in &self.areas {
            frame.stroke(&Path::line(c, a.at), spoke);
        }
        let dashed = Stroke {
            line_dash: LineDash {
                segments: &[3.0, 3.0],
                offset: 0,
            },
            ..spoke
        };
        for (p, q) in &self.links {
            frame.stroke(&Path::line(*p, *q), dashed);
        }
        // The product: a ring that turns signal when it has heard something new, and its heart.
        let ring_colour = if self.heard { pal.signal } else { pal.ink_3 };
        frame.fill(&Path::circle(c, 16.0), pal.sheet);
        frame.stroke(
            &Path::circle(c, 16.0),
            Stroke::default().with_color(ring_colour).with_width(1.5),
        );
        frame.fill(&Path::circle(c, 9.0), pal.ink_2);
        for a in &self.areas {
            let r = if a.on { 15.0 } else { 12.0 };
            let body = Path::circle(a.at, r);
            frame.fill(&body, a.color);
            frame.stroke(
                &body,
                Stroke::default().with_color(pal.sheet).with_width(2.0),
            );
            if a.on {
                frame.stroke(
                    &body,
                    Stroke::default().with_color(pal.signal).with_width(3.0),
                );
            }
            for (k, colour) in a.waiting.iter().enumerate() {
                let t = -PI / 2.0 + k as f32 * PI / 4.0;
                let dot = Point::new(a.at.x + 21.0 * t.cos(), a.at.y + 21.0 * t.sin());
                frame.fill(&Path::circle(dot, 3.5), *colour);
            }
            frame.fill_text(canvas::Text {
                content: a.label.clone(),
                position: Point::new(a.at.x, a.at.y + 28.0),
                color: pal.ink_2,
                size: Pixels(11.0),
                font: theme::SANS,
                align_x: TextAlign::Center,
                ..canvas::Text::default()
            });
        }
        vec![frame.into_geometry()]
    }
}

/// The side panel: the chosen area, or the chosen proposal; else the product and the best of what waits.
fn side_panel<'a>(state: &'a State, data: &'a Maintenance) -> Element<'a, Msg> {
    let pal = theme::palette();
    match &state.selection {
        Selection::Area(id) => {
            if let Some(a) = data.map.areas.iter().find(|a| &a.id == id) {
                return area_panel(state, data, a);
            }
        }
        Selection::Proposal(id) => {
            if let Some(p) = data.proposals.iter().find(|p| &p.id == id) {
                return proposal_box(state, data, p, true);
            }
        }
        Selection::None => {}
    }
    let top: Vec<&Proposal> = waiting_in_order(&data.proposals)
        .into_iter()
        .take(TOP_PROPOSALS)
        .collect();
    let mut lines: Vec<Element<'a, Msg>> = vec![
        text("The product")
            .font(ui::SANS_SEMIBOLD)
            .size(17.0)
            .color(pal.ink)
            .into(),
    ];
    if !data.map.summary.is_empty() {
        lines.push(ui::sans(data.map.summary.clone(), theme::TYPE_UI, pal.ink_2).into());
    }
    if !top.is_empty() {
        let items: Vec<Element<'a, Msg>> = top
            .iter()
            .enumerate()
            .map(|(i, p)| {
                row![
                    ui::faint(format!("{}.", i + 1)),
                    link_button(&p.title, Msg::Select(Selection::Proposal(p.id.clone()))),
                    value_dots(p.value.score),
                ]
                .spacing(8)
                .align_y(Alignment::Center)
                .wrap()
                .into()
            })
            .collect();
        lines.push(column(items).spacing(8).into());
    }
    if !data.map.areas.is_empty() {
        lines.push(ui::faint("Click an area or a proposal to see more.").into());
    }
    column(lines).spacing(12).into()
}

/// The side panel for one area: its health, summary, notes, paths, what it talks to, and what waits in it.
fn area_panel<'a>(state: &'a State, data: &'a Maintenance, a: &'a Area) -> Element<'a, Msg> {
    let pal = theme::palette();
    let mut lines: Vec<Element<'a, Msg>> = vec![
        row![
            disc(health_tone(&a.health).color(), 10.0),
            text(a.name.clone())
                .font(ui::SANS_SEMIBOLD)
                .size(17.0)
                .color(pal.ink),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into(),
        ui::sans(a.summary.clone(), theme::TYPE_UI, pal.ink_2).into(),
    ];
    if !a.notes.is_empty() {
        lines.push(ui::faint(a.notes.clone()).into());
    }
    if !a.paths.is_empty() {
        let chips: Vec<Element<'a, Msg>> = a
            .paths
            .iter()
            .map(|p| {
                container(ui::mono(p.clone(), theme::TYPE_BADGE, pal.ink_2))
                    .padding([2, 8])
                    .style(ui::frame(Some(pal.sheet_2), pal.sheet_2))
                    .into()
            })
            .collect();
        lines.push(row(chips).spacing(8).wrap().into());
    }
    if !a.links.is_empty() {
        let talks: Vec<Element<'a, Msg>> = a
            .links
            .iter()
            .map(|l| {
                let name = data
                    .map
                    .areas
                    .iter()
                    .find(|x| &x.id == l)
                    .map_or(l.clone(), |x| x.name.clone());
                ui::btn_sm(
                    &name,
                    None,
                    ui::Kind::Ghost,
                    Some(Msg::Select(Selection::Area(l.clone()))),
                )
            })
            .collect();
        let mut talk_row: Vec<Element<'a, Msg>> = vec![ui::faint("Talks to").into()];
        talk_row.extend(talks);
        lines.push(
            row(talk_row)
                .spacing(8)
                .align_y(Alignment::Center)
                .wrap()
                .into(),
        );
    }
    let waiting: Vec<&Proposal> = waiting_in_order(&data.proposals)
        .into_iter()
        .filter(|p| area_of(p) == Some(a.id.as_str()))
        .collect();
    if waiting.is_empty() {
        lines.push(ui::faint("Nothing proposed here.").into());
    } else {
        for p in waiting {
            lines.push(proposal_box(state, data, p, state.expert));
        }
    }
    let by = if a.by.is_empty() {
        "a watcher"
    } else {
        a.by.as_str()
    };
    lines.push(ui::faint(format!("Updated {} by {by}", ago(a.updated_ms, now_ms()))).into());
    column(lines).spacing(12).into()
}

/// The last sweep's summary, under a disclosure (closed at first).
fn last_sweep<'a>(state: &'a State, data: &'a Maintenance) -> Element<'a, Msg> {
    let pal = theme::palette();
    let mut lines: Vec<Element<'a, Msg>> = vec![ui::disclosure(
        "What the last sweep said".into(),
        pal.ink_2,
        state.summary_open,
        Msg::ToggleSummary,
    )];
    if state.summary_open {
        lines.push(ui::sans(data.sweep.last_summary.clone(), theme::TYPE_UI, pal.ink_2).into());
    }
    column(lines).spacing(8).into()
}

/// The proposals waiting for the owner, best first, each in its own box.
fn proposals_card<'a>(state: &'a State, data: &'a Maintenance) -> Element<'a, Msg> {
    let waiting = waiting_in_order(&data.proposals);
    let body: Element<'a, Msg> = if waiting.is_empty() {
        ui::empty(
            "search",
            "Nothing proposed yet",
            if data.watchers.is_empty() {
                "Add an agent to the project (a planner or a researcher) so someone watches it."
            } else {
                "The watchers propose after they read what comes in."
            },
        )
    } else {
        let items: Vec<Element<'a, Msg>> = waiting
            .into_iter()
            .map(|p| proposal_box(state, data, p, state.expert))
            .collect();
        column(items).spacing(12).into()
    };
    ui::card("Proposals", body)
}

/// One proposal in its box: its kind, title and value; what would change, its risk, size and area (and "later");
/// its details when open; then the three answers (Do it, Later, No), or what became of it.
fn proposal_box<'a>(
    state: &'a State,
    data: &'a Maintenance,
    p: &'a Proposal,
    open: bool,
) -> Element<'a, Msg> {
    let pal = theme::palette();
    let waiting = is_waiting(p);
    let selected = matches!(&state.selection, Selection::Proposal(id) if id == &p.id);
    let area_name = area_of(p)
        .and_then(|a| data.map.areas.iter().find(|x| x.id == a))
        .map_or(String::new(), |a| a.name.clone());
    let mut lines: Vec<Element<'a, Msg>> = vec![
        row![
            ui::badge::<Msg>(&kind_label(&p.kind), kind_tone(&p.kind)),
            button(
                text(p.title.clone())
                    .font(ui::SANS_SEMIBOLD)
                    .size(theme::TYPE_ITEM)
                    .color(pal.ink),
            )
            .padding(0.0)
            .width(Length::Fill)
            .style(|_: &Theme, _: button::Status| button::Style {
                background: None,
                text_color: theme::palette().ink,
                border: Border::default(),
                ..Default::default()
            })
            .on_press(Msg::Select(if selected {
                Selection::None
            } else {
                Selection::Proposal(p.id.clone())
            })),
            value_dots(p.value.score),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into(),
        ui::sans(
            if p.impact.changes.is_empty() {
                p.summary.clone()
            } else {
                p.impact.changes.clone()
            },
            theme::TYPE_UI,
            pal.ink_2,
        )
        .into(),
    ];
    let mut weigh: Vec<Element<'a, Msg>> = vec![
        ui::sans(
            format!("{} risk", p.impact.risk),
            theme::TYPE_META,
            risk_tone(&p.impact.risk).color(),
        )
        .into(),
        ui::faint(size_short(&p.effort.size)).into(),
    ];
    if !area_name.is_empty() {
        weigh.push(ui::faint(area_name).into());
    }
    if p.state == "later" {
        weigh.push(ui::badge::<Msg>("later", ui::Tone::Warn));
    }
    lines.push(
        row(weigh)
            .spacing(12)
            .align_y(Alignment::Center)
            .wrap()
            .into(),
    );
    if open {
        let mut details: Vec<Element<'a, Msg>> =
            vec![ui::sans(p.summary.clone(), theme::TYPE_UI, pal.ink).into()];
        if !p.impact.users.is_empty() {
            details.push(kfield("Who it touches", p.impact.users.clone()));
        }
        details.push(kfield("Why it is worth it", p.value.why.clone()));
        if !p.impact.risk_note.is_empty() {
            details.push(kfield("Risk", p.impact.risk_note.clone()));
        }
        if let Some(i) = &p.instrument {
            details.push(kfield(
                "Data it would give",
                format!("{} ({}): {}", i.name, i.kind, i.data),
            ));
        }
        if !p.evidence.is_empty() {
            let items: Vec<Element<'a, Msg>> = p
                .evidence
                .iter()
                .map(|e| {
                    let line = evidence_text(e);
                    ui::sans(format!("• {line}"), theme::TYPE_UI, pal.ink_2).into()
                })
                .collect();
            details.push(
                column![ui::label("Evidence"), column(items).spacing(4)]
                    .spacing(6)
                    .into(),
            );
        }
        if !p.acceptance.is_empty() {
            details.push(kfield("Done when", p.acceptance.clone()));
        }
        details.push(
            ui::faint(format!(
                "Proposed by {} {} · for a {}",
                p.by,
                ago(p.created_ms, now_ms()),
                p.role
            ))
            .into(),
        );
        lines.push(
            column![ui::hairline(), column(details).spacing(8)]
                .spacing(12)
                .into(),
        );
    }
    lines.push(answers(state, p, waiting));
    container(column(lines).spacing(8))
        .padding(16)
        .width(Length::Fill)
        .style(ui::frame(Some(pal.sheet), pal.rule))
        .into()
}

/// Do it, Later and No for a waiting proposal; what became of it otherwise.
fn answers<'a>(state: &'a State, p: &'a Proposal, waiting: bool) -> Element<'a, Msg> {
    if !waiting {
        return ui::sans(outcome_line(p), theme::TYPE_META, theme::palette().ink_2).into();
    }
    let busy = state.busy;
    let mut buttons: Vec<Element<'a, Msg>> = vec![ui::btn_sm(
        "Do it",
        None,
        ui::Kind::Primary,
        (!busy).then_some(Msg::OpenApprove(p.id.clone())),
    )];
    if p.state != "later" {
        buttons.push(ui::btn_sm(
            "Later",
            None,
            ui::Kind::Secondary,
            (!busy).then_some(Msg::Later(p.id.clone())),
        ));
    }
    buttons.push(ui::btn_sm(
        "No",
        None,
        ui::Kind::Ghost,
        (!busy).then_some(Msg::OpenDecline(p.id.clone())),
    ));
    row(buttons).spacing(8).wrap().into()
}

/// Data the watchers lack: each waiting proposal that needs an instrument, and what it would give them.
fn lacking_card<'a>(list: &[&'a Proposal]) -> Element<'a, Msg> {
    let pal = theme::palette();
    let items: Vec<Element<'a, Msg>> = list
        .iter()
        .map(|p| {
            let (name, data) = match &p.instrument {
                Some(i) => (
                    if i.name.is_empty() {
                        p.title.clone()
                    } else {
                        i.name.clone()
                    },
                    i.data.clone(),
                ),
                None => (p.title.clone(), p.summary.clone()),
            };
            ui::sans(format!("• {name} — {data}"), theme::TYPE_UI, pal.ink).into()
        })
        .collect();
    ui::card("Data the watchers lack", column(items).spacing(8).into())
}

/// The inbox: the signals, newest first, each with its kind, its source and its age; the ingest address and the
/// add-a-signal action in the head.
fn inbox_card<'a>(state: &'a State, data: &'a Maintenance) -> Element<'a, Msg> {
    let pal = theme::palette();
    let shown = if state.expert {
        SHOWN_SIGNALS_EXPERT
    } else {
        SHOWN_SIGNALS
    };
    let now = now_ms();
    let mut actions: Vec<Element<'a, Msg>> = vec![
        ui::btn_sm(
            "Add a signal",
            Some("plus"),
            ui::Kind::Secondary,
            Some(Msg::OpenSignal),
        ),
        ui::btn_sm(
            if data.sources.ingest {
                "New ingest address"
            } else {
                "Ingest address"
            },
            None,
            ui::Kind::Secondary,
            Some(Msg::NewIngest),
        ),
    ];
    if data.sources.ingest {
        actions.push(ui::btn_sm(
            "Stop it",
            None,
            ui::Kind::Ghost,
            Some(Msg::StopIngest),
        ));
    }
    let title = if data.unseen > 0 {
        format!("Inbox ({}, {} new)", data.signals_total, data.unseen)
    } else {
        format!("Inbox ({})", data.signals_total)
    };
    let body: Element<'a, Msg> = if data.signals.is_empty() {
        ui::empty(
            "search",
            "Nothing has come in yet.",
            "Add what users told you, or give your app or error tracker the ingest address.",
        )
    } else {
        let mut rows: Vec<Element<'a, Msg>> = Vec::new();
        for s in data.signals.iter().take(shown) {
            if !rows.is_empty() {
                rows.push(ui::hairline());
            }
            let tone = match s.kind.as_str() {
                "error" => ui::Tone::Danger,
                "report" => ui::Tone::Warn,
                _ => ui::Tone::Muted,
            };
            let mut meta = s.from.clone();
            if let Some(env) = s.environment.as_deref().filter(|e| !e.is_empty()) {
                meta.push_str(&format!(" · {env}"));
            }
            meta.push_str(&format!(" · {}", ago(s.at_ms, now)));
            let mut ends: Vec<Element<'a, Msg>> = Vec::new();
            if let Some(url) = s.url.as_deref().filter(|u| !u.is_empty()) {
                ends.push(ui::link("open", Msg::OpenUrl(url.to_string()), false));
            }
            ends.push(ui::btn_sm(
                "",
                Some("trash-2"),
                ui::Kind::Ghost,
                Some(Msg::DeleteSignal(s.id.clone())),
            ));
            rows.push(
                row![
                    ui::badge::<Msg>(&s.kind, tone),
                    column![
                        ui::sans(signal_line(s), theme::TYPE_UI, pal.ink),
                        ui::faint(meta)
                    ]
                    .spacing(2)
                    .width(Length::Fill),
                    row(ends).spacing(8).align_y(Alignment::Center),
                ]
                .spacing(12)
                .align_y(Alignment::Center)
                .into(),
            );
        }
        if data.signals_total > shown as i64 {
            rows.push(
                ui::faint(format!(
                    "Showing the newest {shown} of {}.",
                    data.signals_total
                ))
                .into(),
            );
        }
        column(rows).spacing(10).into()
    };
    ui::card_with(&title, actions, body)
}

/// What the watchers read: the inbox, the connectors and the MCP servers, or the note that there are none.
fn reads_card<'a>(data: &'a Maintenance) -> Element<'a, Msg> {
    let pal = theme::palette();
    let s = &data.sources;
    let mut lines: Vec<Element<'a, Msg>> = Vec::new();
    let inbox = if s.ingest {
        format!("The inbox: {} signals, and its ingest address", s.inbox)
    } else {
        format!("The inbox: {} signals", s.inbox)
    };
    lines.push(ui::sans(format!("• {inbox}"), theme::TYPE_UI, pal.ink).into());
    for c in &s.connectors {
        let label = if c.label.is_empty() {
            c.id.clone()
        } else {
            c.label.clone()
        };
        lines.push(
            ui::sans(
                format!("• {label} ({})", c.category),
                theme::TYPE_UI,
                pal.ink,
            )
            .into(),
        );
    }
    for m in &s.mcp {
        let name = if m.name.is_empty() {
            m.id.clone()
        } else {
            m.name.clone()
        };
        lines.push(ui::sans(format!("• The MCP server {name}"), theme::TYPE_UI, pal.ink).into());
    }
    if s.connectors.is_empty() && s.mcp.is_empty() {
        lines.push(
            row![
                ui::faint("No analytics, error or support tools are connected yet."),
                ui::text_link("Add an MCP server", Msg::Go(Id::Mcp)),
                ui::faint(" or "),
                ui::text_link("a connector", Msg::Go(Id::Tools)),
                ui::faint(", or let the watchers propose one."),
            ]
            .align_y(Alignment::Center)
            .wrap()
            .into(),
        );
    }
    ui::card("What the watchers read", column(lines).spacing(8).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal(id: &str, state: &str, score: i64, risk: &str, created_ms: i64) -> Proposal {
        Proposal {
            id: id.into(),
            state: state.into(),
            value: Worth {
                score,
                why: String::new(),
            },
            impact: Impact {
                risk: risk.into(),
                ..Default::default()
            },
            created_ms,
            ..Default::default()
        }
    }

    fn ids(list: &[&Proposal]) -> Vec<String> {
        list.iter().map(|p| p.id.clone()).collect()
    }

    #[test]
    fn the_waiting_proposals_go_best_first_and_the_put_off_ones_last() {
        let props = [
            proposal("a", "proposed", 3, "low", 100),
            proposal("b", "proposed", 5, "high", 50),
            proposal("c", "proposed", 5, "low", 10),
            proposal("d", "later", 3, "low", 200),
            proposal("e", "proposed", 3, "low", 300),
            proposal("f", "approved", 5, "low", 999),
        ];
        assert_eq!(
            ids(&waiting_in_order(&props)),
            ["c", "b", "e", "a", "d"],
            "value first, then the lower risk, then the ones not put off, then the newest"
        );
    }

    #[test]
    fn the_waiting_count_and_the_decided_split() {
        let props = [
            proposal("a", "proposed", 1, "low", 1),
            proposal("b", "later", 1, "low", 2),
            proposal("c", "approved", 1, "low", 3),
            proposal("d", "declined", 1, "low", 4),
            proposal("e", "withdrawn", 1, "low", 5),
        ];
        assert_eq!(waiting_count(&props), 2);
        assert_eq!(ids(&decided_of(&props)), ["c", "d", "e"]);
        assert_eq!(
            waiting_in_order(&props).len() + decided_of(&props).len(),
            props.len()
        );
    }

    #[test]
    fn health_has_a_colour_each() {
        assert_eq!(health_tone("good"), ui::Tone::Ok);
        assert_eq!(health_tone("watch"), ui::Tone::Warn);
        assert_eq!(health_tone("poor"), ui::Tone::Danger);
        assert_eq!(health_tone("unknown"), ui::Tone::Muted);
        assert_eq!(health_tone(""), ui::Tone::Muted);
    }

    #[test]
    fn the_interval_has_its_words() {
        assert_eq!(every_text(0), "only when you ask");
        assert_eq!(every_text(1), "every hour");
        assert_eq!(every_text(24), "every 24 h");
    }

    #[test]
    fn ago_reads_as_the_web_does() {
        let now: i64 = 1_800_000_000_000;
        assert_eq!(ago(now - 20_000, now), "just now");
        assert_eq!(ago(now - 5 * 60_000, now), "5 min ago");
        assert_eq!(ago(now - 3 * 3_600_000, now), "3 h ago");
        assert_eq!(ago(now - 2 * 86_400_000, now), "2 d ago");
        assert_eq!(ago(0, now), "—");
    }

    #[test]
    fn the_headline_counts_what_waits() {
        assert_eq!(headline(0, 0, false), "No sweep yet");
        assert_eq!(headline(0, 3, false), "Nothing waits for you");
        assert_eq!(headline(1, 3, false), "One proposal waits for you");
        assert_eq!(headline(2, 3, true), "2 proposals wait for you");
    }

    #[test]
    fn a_signal_reads_as_its_title_or_the_start_of_its_text() {
        let titled = Signal {
            title: "Checkout fails".into(),
            text: "long text".into(),
            ..Default::default()
        };
        assert_eq!(signal_line(&titled), "Checkout fails");
        let untitled = Signal {
            text: "word ".repeat(60),
            ..Default::default()
        };
        let line = signal_line(&untitled);
        assert!(line.chars().count() <= 120, "cut to 120 characters");
        assert!(line.ends_with('…'));
    }

    #[test]
    fn the_map_links_each_pair_once() {
        let area = |id: &str, links: &[&str]| Area {
            id: id.into(),
            links: links.iter().map(|l| l.to_string()).collect(),
            ..Default::default()
        };
        let areas = [area("a", &["b", "x"]), area("b", &["a"]), area("c", &["c"])];
        assert_eq!(map_links(&areas), vec![(0, 1)]);
    }
}
