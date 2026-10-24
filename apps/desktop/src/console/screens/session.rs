//! The session page (the web's `session` view): one session by its id (`s12`), or the newest session on a VM by its CID
//! (`105`, the link an agent had). Its header (the state, the stop button), its facts (the seat, the task, the project,
//! the model, the usage and the times; the expert view adds the node, the VM, the run, the engine, the attempt, the
//! compactions and the micro-dollars), and two tabs: the transcript (the log of its work, oldest first, the newest rows)
//! and the context (what its last model call sent, by part and by tool). Stop ends the session: a long-lived one (a
//! planner or the orchestrator) asks first.

use iced::widget::{Row, Space, column, container, row};
use iced::{Alignment, Background, Color, Element, Length, Task, Theme};

use super::Id;
use super::budget_model;
use super::session_model::{self as model, PLAZA_PROJECT, Part};
use super::work_ui::{self, Col, EDGE};
use crate::console::chat_tools::tokens_text;
use crate::console::client::Client;
use crate::console::events;
use crate::console::work_api::{
    self, Context as SessionContext, ContextPart, Session, SessionKind, TranscriptEntry,
};
use crate::console::{Ctx, client, scope, ui};
use crate::theme;

/// Log entries read per request (the hub's page is at most 1000).
const LOG_PAGE: u32 = 500;
/// The newest log entries kept while paging; older ones are dropped as new pages arrive.
const LOG_HELD: usize = 1000;
/// The newest log rows drawn.
const LOG_ROWS: usize = 300;
/// A log entry's raw data, as the expert view shows it, at most this many characters.
const RAW_CHARS: usize = 2000;
/// A handover, as the facts show it, at most this many characters.
const HANDOVER_CHARS: usize = 600;
/// The size of a log row's words.
const LOG_TEXT: f32 = 14.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Transcript,
    Context,
}

/// One session's log as read so far: the entries held, oldest first, the count of older ones dropped, and the sequence
/// number the next read starts after. `owner` is `<id>@<cid>`: a new session on the VM starts a log of its own.
#[derive(Debug, Clone, Default)]
pub struct Log {
    owner: String,
    entries: Vec<TranscriptEntry>,
    dropped: usize,
    cursor: u64,
}

/// What one load read: the session (none when no session runs on the VM), its context when the Context tab asked for
/// it, the log entries after the ones held, and the first read that failed after the session was read.
#[derive(Debug, Clone, Default)]
pub struct Fetched {
    session: Option<Session>,
    context: Option<SessionContext>,
    /// The log this read is of (`<id>@<cid>`), and whether it starts a log of its own.
    owner: String,
    fresh_log: bool,
    entries: Vec<TranscriptEntry>,
    /// The sequence number of the last entry read.
    cursor: u64,
    failure: Option<String>,
}

#[derive(Debug, Default)]
pub struct State {
    /// What the page opened on: a session's id (`s12`), or a VM's CID (`105`).
    reference: String,
    /// The session as last read; None when no session runs on the VM.
    session: Option<Session>,
    /// The first load has answered (until then the page is blank, as on the web).
    loaded: bool,
    /// Why the last load failed, as a line for the page; None when it did not.
    failure: Option<String>,
    context: Option<SessionContext>,
    log: Option<Log>,
    tab: Tab,
    expert: bool,
    stopping: bool,
    /// The stop of a long-lived session waits for the owner's confirmation.
    confirm_stop: bool,
    /// A line for the last stop ("Session stopped", or why it was refused).
    note: Option<String>,
    /// A load is in flight; a frame that comes meanwhile only asks for one more.
    loading: bool,
    again: bool,
}

#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to another screen.
    Go(Id),
    Loaded(Result<Fetched, String>),
    Tab(Tab),
    Expert(bool),
    /// Stop pressed: a step stops at once, a long-lived session asks first.
    AskStop,
    CancelStop,
    ConfirmStop,
    Stopped(Result<Session, String>),
    /// A hub event: a session or a transcript frame of this page's session reloads it.
    Event(events::Event),
}

/// The page of one session, by its id or by its VM's CID.
pub fn opened(reference: String) -> State {
    State {
        reference,
        ..Default::default()
    }
}

pub fn go(msg: &Msg) -> Option<Id> {
    match msg {
        Msg::Go(id) => Some(id.clone()),
        _ => None,
    }
}

/// The hub's events while the page is open: its session's frames and its transcript's.
pub fn subscription(ctx: &Ctx) -> iced::Subscription<Msg> {
    events::subscription(&ctx.client).map(Msg::Event)
}

/// Reads the session, its context on the Context tab, and the log after the entries held (called when the page opens,
/// on every poll tick and on a hub event).
pub fn load(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    state.loading = true;
    let client = ctx.client.clone();
    let reference = state.reference.clone();
    let tab = state.tab;
    let held = state
        .log
        .as_ref()
        .map(|log| (log.owner.clone(), log.cursor));
    client::task(move || fetch(&client, &reference, tab, held), Msg::Loaded)
}

/// The reads of one load. The session comes first: its owner says whether the log held is still its log, and then the
/// log reads on from the last entry held, a page at a time, until the end.
fn fetch(
    client: &Client,
    reference: &str,
    tab: Tab,
    held: Option<(String, u64)>,
) -> Result<Fetched, String> {
    let session = if model::is_cid_ref(reference) {
        let all = work_api::sessions(client, None, None, false)
            .map_err(|e| format!("Could not load the session: {e}"))?;
        reference
            .parse::<u64>()
            .ok()
            .and_then(|cid| model::newest_with_cid(&all, cid).cloned())
    } else {
        Some(
            work_api::session(client, reference)
                .map_err(|e| format!("Could not load the session: {e}"))?,
        )
    };
    let Some(session) = session else {
        return Ok(Fetched::default());
    };
    let owner = format!(
        "{}@{}",
        session.id,
        session.cid.map_or(String::new(), |cid| cid.to_string())
    );
    let fresh_log = held.as_ref().is_none_or(|(prev, _)| *prev != owner);
    let mut after = match &held {
        Some((_, cursor)) if !fresh_log => *cursor,
        _ => 0,
    };
    let mut fetched = Fetched {
        context: None,
        owner,
        fresh_log,
        cursor: after,
        ..Default::default()
    };
    if tab == Tab::Context {
        match work_api::context(client, &session.id) {
            Ok(context) => fetched.context = Some(context),
            Err(e) => fetched.failure = Some(format!("Could not load the context: {e}")),
        }
    }
    if session.cid.is_some() {
        loop {
            let page = match work_api::transcript(client, &session.id, after, LOG_PAGE) {
                Ok(page) => page,
                Err(e) => {
                    fetched.failure = Some(format!("Could not load the transcript: {e}"));
                    break;
                }
            };
            let mut fresh = 0;
            for entry in &page {
                if entry.seq > after {
                    after = entry.seq;
                    fetched.entries.push(entry.clone());
                    fresh += 1;
                }
            }
            fetched.cursor = after;
            if page.len() < LOG_PAGE as usize || fresh == 0 {
                break;
            }
        }
    }
    fetched.session = Some(session);
    Ok(fetched)
}

/// A load landed: one more runs if frames came in while it was in flight.
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
        Msg::Loaded(Ok(fetched)) => {
            state.loaded = true;
            state.failure = fetched.failure.clone();
            match fetched.session.clone() {
                None => {
                    state.session = None;
                    state.context = None;
                    state.log = None;
                }
                Some(session) => {
                    let mut log = match state.log.take() {
                        Some(log) if !fetched.fresh_log && log.owner == fetched.owner => log,
                        _ => {
                            state.context = None;
                            Log {
                                owner: fetched.owner.clone(),
                                ..Default::default()
                            }
                        }
                    };
                    log.entries.extend(fetched.entries);
                    log.cursor = log.cursor.max(fetched.cursor);
                    let over = log.entries.len().saturating_sub(LOG_HELD);
                    if over > 0 {
                        log.entries.drain(..over);
                        log.dropped += over;
                    }
                    if let Some(context) = fetched.context {
                        state.context = Some(context);
                    }
                    state.log = Some(log);
                    state.session = Some(session);
                }
            }
            landed(state, ctx)
        }
        Msg::Loaded(Err(message)) => {
            state.loaded = true;
            state.failure = Some(message);
            landed(state, ctx)
        }
        Msg::Tab(tab) => {
            state.tab = tab;
            load(state, ctx)
        }
        Msg::Expert(on) => {
            state.expert = on;
            Task::none()
        }
        Msg::AskStop => match &state.session {
            Some(session) if session.kind == SessionKind::Long => {
                state.confirm_stop = true;
                Task::none()
            }
            Some(_) => stop(state, ctx),
            None => Task::none(),
        },
        Msg::CancelStop => {
            state.confirm_stop = false;
            Task::none()
        }
        Msg::ConfirmStop => {
            state.confirm_stop = false;
            stop(state, ctx)
        }
        Msg::Stopped(Ok(session)) => {
            state.stopping = false;
            state.note = Some("Session stopped".into());
            state.session = Some(session);
            load(state, ctx)
        }
        Msg::Stopped(Err(message)) => {
            state.stopping = false;
            state.note = Some(message);
            Task::none()
        }
        Msg::Event(event) => {
            let relevant = match (event.kind.as_str(), &state.session) {
                ("session", _) if model::is_cid_ref(&state.reference) => true,
                ("session", Some(session)) => {
                    event.data["id"].as_str() == Some(session.id.as_str())
                }
                ("transcript", Some(session)) => event.data["cid"].as_u64() == session.cid,
                _ => false,
            };
            if !relevant {
                return Task::none();
            }
            if state.loading {
                state.again = true;
                return Task::none();
            }
            load(state, ctx)
        }
    }
}

/// Stops the shown session: it ends with its handover, and its seat goes again.
fn stop(state: &mut State, ctx: &Ctx) -> Task<Msg> {
    let Some(session) = &state.session else {
        return Task::none();
    };
    state.stopping = true;
    let client = ctx.client.clone();
    let id = session.id.clone();
    client::task(move || work_api::stop_session(&client, &id), Msg::Stopped)
}

pub fn view<'a>(state: &'a State) -> Element<'a, Msg> {
    let back = ui::text_link("All sessions", Msg::Go(Id::Sessions));
    if !state.loaded {
        return column![back, ui::loading()].spacing(16).into();
    }
    let Some(session) = &state.session else {
        let empty = match &state.failure {
            Some(message) => ui::empty("alert", "Could not load the session", message),
            None => ui::empty(
                "agents",
                &format!("No session runs on VM {} now", state.reference),
                "Its last session is in the Sessions list.",
            ),
        };
        return column![back, empty].spacing(16).into();
    };
    let page = column![
        back,
        header(state, session),
        ui::card("Details", facts(state.expert, session)),
        failure_line(state),
        ui::tabs(
            &[(Tab::Transcript, "Transcript"), (Tab::Context, "Context")],
            state.tab,
            Msg::Tab,
        ),
        match state.tab {
            Tab::Transcript => transcript(state),
            Tab::Context => context_view(state),
        },
    ]
    .spacing(16)
    .width(Length::Fill);
    if state.confirm_stop {
        ui::with_modal(page.into(), Some(stop_dialog()))
    } else {
        page.into()
    }
}

/// The page head: the session's heading, its id and state, the stop button and the expert switch.
fn header<'a>(state: &'a State, session: &Session) -> Element<'a, Msg> {
    let mut line = format!("{} · {}", session.id, model::state_label(session.state));
    if let Some(reason) = model::paused_text(session.paused_by) {
        line.push_str(" · ");
        line.push_str(reason);
    }
    let stop_press = (!model::is_final(session.state) && !state.stopping).then_some(Msg::AskStop);
    let mut actions: Vec<Element<'a, Msg>> = vec![
        ui::badge(
            model::state_label(session.state),
            model::state_tone(session.state),
        ),
        ui::btn("Stop", None, ui::Kind::DangerOutline, stop_press),
        work_ui::switch("Expert", state.expert, Msg::Expert),
    ];
    if let Some(note) = &state.note {
        actions.insert(0, ui::faint(note.clone()).into());
    }
    ui::view_head_with_intro(&model::heading(session), &line, actions)
}

/// The stop confirmation of a planner or the orchestrator: what stopping ends, then Keep running or Stop.
fn stop_dialog<'a>() -> Element<'a, Msg> {
    ui::modal(
        "Stop this session?",
        ui::prose("A planner or the orchestrator runs as one long-lived session. Stopping it ends its work now.")
        .into(),
        vec![
            ui::btn("Keep running", None, ui::Kind::Ghost, Some(Msg::CancelStop)),
            ui::btn("Stop", None, ui::Kind::Danger, Some(Msg::ConfirmStop)),
        ],
        Msg::CancelStop,
        440.0,
    )
}

/// A failed read as one line above the page: the page stays, the line says what did not load.
fn failure_line<'a>(state: &'a State) -> Element<'a, Msg> {
    match &state.failure {
        Some(message) => ui::error_line(message),
        None => column![].into(),
    }
}

/// The facts a session leads with, in rows of three: the seat, the task, the project, the model, the usage, the times,
/// the verdict, the outcome and the handover; the expert view adds the raw ids and the micro-dollars.
fn facts<'a>(expert: bool, s: &Session) -> Element<'a, Msg> {
    let u = &s.usage;
    let mut items: Vec<Element<'a, Msg>> = vec![
        work_ui::fact("Seat", ink(model::seat_text(s))),
        work_ui::fact(
            "Task",
            match &s.task {
                Some(task) => ui::text_link(task, Msg::Go(Id::Task(task.clone()))),
                None => ui::faint("No task").into(),
            },
        ),
        work_ui::fact(
            "Project",
            if s.project == PLAZA_PROJECT {
                ink("The plaza".into())
            } else {
                ui::text_link(
                    &scope::title(&s.project),
                    Msg::Go(Id::Project(s.project.clone())),
                )
            },
        ),
        work_ui::fact(
            "Model",
            match &s.model {
                Some(model) => ui::mono(model.clone(), 13.0, theme::palette().ink).into(),
                None => ui::faint("—").into(),
            },
        ),
        work_ui::fact(
            "Usage",
            ink(format!(
                "{} · {} cached · {} · {}",
                model::usage_text(u),
                tokens_text(u.tokens_cached),
                model::cost_text(u.cost_micros),
                model::count_text(u.tool_calls, "tool call", "tool calls"),
            )),
        ),
        work_ui::fact(
            "Started",
            ink(ui::ago(s.started_ms.unwrap_or(s.created_ms))),
        ),
    ];
    if let Some(ended) = s.ended_ms {
        items.push(work_ui::fact("Ended", ink(ui::ago(ended))));
    }
    if let Some(verdict) = s.verdict {
        items.push(work_ui::fact(
            "Verdict",
            ink(if verdict { "Passed" } else { "Not passed" }.into()),
        ));
    }
    if let Some(outcome) = &s.outcome {
        items.push(work_ui::fact("Outcome", ink(outcome.clone())));
    }
    if let Some(handover) = s.handover.as_ref().filter(|_| model::is_final(s.state)) {
        items.push(work_ui::fact(
            "Handover",
            ink(ui::ellipsize(handover, HANDOVER_CHARS)),
        ));
    }
    if expert {
        items.extend([
            work_ui::fact("Node", ink(s.node.clone().unwrap_or_else(|| "—".into()))),
            work_ui::fact(
                "VM",
                ink(s.cid.map_or_else(|| "—".into(), |cid| cid.to_string())),
            ),
            work_ui::fact("Run", ink(s.run.clone().unwrap_or_else(|| "—".into()))),
            work_ui::fact("Engine", ink(s.engine.clone())),
            work_ui::fact("Attempt", ink(s.attempt.to_string())),
            work_ui::fact("Compactions", ink(s.compactions.to_string())),
            work_ui::fact("Model calls", ink(u.model_calls.to_string())),
            work_ui::fact(
                "Cost, micro-dollars",
                ink(format!(
                    "{} µ$",
                    budget_model::grouped(u.cost_micros as i64)
                )),
            ),
        ]);
    }
    work_ui::facts(items, 3)
}

/// A line of the page's ink in the sans voice.
fn ink<'a>(words: String) -> Element<'a, Msg> {
    ui::sans(words, 14.0, theme::palette().ink).into()
}

/// The transcript: the newest rows of the log, oldest first, and a note for the entries that are read but not drawn.
fn transcript<'a>(state: &'a State) -> Element<'a, Msg> {
    let (entries, dropped): (&[TranscriptEntry], usize) = match &state.log {
        Some(log) => (&log.entries, log.dropped),
        None => (&[], 0),
    };
    let shown = &entries[entries.len().saturating_sub(LOG_ROWS)..];
    let earlier = dropped + entries.len() - shown.len();
    let mut lines: Vec<Element<'a, Msg>> = Vec::new();
    if earlier > 0 {
        lines.push(ui::faint(format!("{earlier} earlier entries are not shown.")).into());
    }
    if shown.is_empty() {
        let session = state.session.as_ref();
        let words = match session {
            Some(s) if s.cid.is_none() => "Nothing has run in this session yet.",
            _ => "No entries yet.",
        };
        lines.push(ink(words.into()));
    }
    lines.extend(shown.iter().map(|e| log_row(e, state.expert)));
    column(lines).spacing(14).width(Length::Fill).into()
}

/// One log row: its time, its label (with a note), its text, and in the expert view its sequence number and raw data.
fn log_row<'a>(e: &TranscriptEntry, expert: bool) -> Element<'a, Msg> {
    let line = model::transcript_line(&e.kind, &e.data);
    let palette = theme::palette();
    let label_color = if line.alert {
        palette.crit
    } else if line.known {
        palette.ink
    } else {
        palette.ink_3
    };
    let mut head = row![ui::sans(line.label.clone(), LOG_TEXT, label_color)]
        .spacing(10)
        .align_y(Alignment::Center);
    if !line.note.is_empty() {
        head = head.push(ui::faint(line.note.clone()));
    }
    if expert {
        head = head.push(ui::mono(format!("#{}", e.seq), 12.0, palette.ink_3));
    }
    let mut body = column![head].spacing(4).width(Length::Fill);
    if !line.text.is_empty() {
        let words: Element<'a, Msg> = if line.mono {
            ui::mono(line.text.clone(), 13.0, palette.ink).into()
        } else {
            ui::prose(line.text.clone()).into()
        };
        body = body.push(words);
    }
    if expert {
        let raw = ui::ellipsize(&e.data.to_string(), RAW_CHARS);
        body = body.push(ui::mono(raw, 12.0, palette.ink_3));
    }
    row![
        container(ui::mono(model::clock_text(e.at_ms), 12.0, palette.ink_3))
            .width(Length::Fixed(80.0)),
        body,
    ]
    .spacing(12)
    .into()
}

/// The context: the fill bar, the parts and the tools; before the first model call, a line saying so.
fn context_view<'a>(state: &'a State) -> Element<'a, Msg> {
    let Some(context) = &state.context else {
        return ink(if state.failure.is_some() {
            "No context to show.".into()
        } else {
            "Loading…".into()
        });
    };
    if !model::has_call(context) {
        return ui::empty(
            "agents",
            "No model call yet",
            "The context shows here once the session has made its first call.",
        );
    }
    let window = context.window.filter(|w| *w > 0);
    let fill = model::context_fill(context).unwrap_or(0.0);
    let caption = match window {
        Some(w) => format!(
            "{} / {} · {}%",
            tokens_text(context.used),
            tokens_text(w),
            (fill * 100.0).round()
        ),
        None => format!("{} used", tokens_text(context.used)),
    };
    let parts = model::ordered_parts(&context.parts);
    let part_rows: Vec<Element<'a, Msg>> = parts
        .iter()
        .map(|(part, tokens)| {
            let colour = part_colour(*part);
            let swatch = container(Space::new())
                .width(Length::Fixed(10.0))
                .height(Length::Fixed(10.0))
                .style(move |_: &Theme| container::Style {
                    background: Some(Background::Color(colour)),
                    ..Default::default()
                });
            let name = row![swatch, ink(part.name().into())]
                .spacing(8)
                .align_y(Alignment::Center);
            work_ui::grid_row(
                &[Col::flex(4), Col::right(2), Col::right(2)],
                vec![
                    name.into(),
                    ink(tokens_text(*tokens)),
                    ink(share(*tokens, context.used)),
                ],
                EDGE,
            )
        })
        .collect();
    let tool_rows: Vec<Element<'a, Msg>> = model::tools_by_result(&context.tools)
        .into_iter()
        .map(|tool| {
            let errors: Element<'a, Msg> = if tool.errors > 0 {
                ui::sans(tool.errors.to_string(), LOG_TEXT, theme::palette().crit).into()
            } else {
                ink(tool.errors.to_string())
            };
            work_ui::grid_row(
                &[Col::flex(4), Col::right(2), Col::right(2), Col::right(2)],
                vec![
                    ui::mono(tool.tool, 13.0, theme::palette().ink).into(),
                    ink(tool.calls.to_string()),
                    errors,
                    ink(format!("{} result", tokens_text(tool.result_tokens))),
                ],
                EDGE,
            )
        })
        .collect();
    let tools = if tool_rows.is_empty() {
        ink("No tool calls yet".into())
    } else {
        work_ui::table(
            &[Col::flex(4), Col::right(2), Col::right(2), Col::right(2)],
            &["Tool", "Calls", "Errors", "Result"],
            tool_rows,
            EDGE,
        )
    };
    column![
        ui::card(
            "Context",
            column![
                bar(&context.parts, context.used, window),
                ui::faint(caption),
            ]
            .spacing(10)
            .into(),
        ),
        ui::card(
            "Parts",
            work_ui::table(
                &[Col::flex(4), Col::right(2), Col::right(2)],
                &["Part", "Tokens", "Share"],
                part_rows,
                EDGE,
            ),
        ),
        ui::card("Tools", tools),
        ui::faint(format!(
            "{} · {}",
            model::count_text(context.calls, "model call", "model calls"),
            model::count_text(u64::from(context.compactions), "compaction", "compactions"),
        )),
    ]
    .spacing(16)
    .width(Length::Fill)
    .into()
}

/// A part's share of what the last call sent, as the web's `shareText`: "0%" when nothing was sent.
fn share(part: u64, total: u64) -> String {
    if total == 0 {
        return "0%".into();
    }
    format!("{}%", ((part as f64 / total as f64) * 100.0).round())
}

/// The fill bar: the parts in their order, then the window's room left, on a track.
fn bar<'a>(parts: &[ContextPart], used: u64, window: Option<u64>) -> Element<'a, Msg> {
    let total = window.unwrap_or(0).max(used);
    let mut segments: Vec<Element<'a, Msg>> = Vec::new();
    if total > 0 {
        for (part, tokens) in model::ordered_parts(parts) {
            if tokens > 0 {
                segments.push(segment(portion(tokens, total), part_colour(part)));
            }
        }
        if let Some(window) = window.filter(|w| *w > used) {
            segments.push(segment(
                portion(window - used, total),
                theme::palette().rule_faint,
            ));
        }
    }
    container(
        Row::with_children(segments)
            .width(Length::Fill)
            .height(Length::Fixed(10.0)),
    )
    .width(Length::Fill)
    .height(Length::Fixed(10.0))
    .clip(true)
    .style(|_: &Theme| container::Style {
        background: Some(Background::Color(theme::palette().rule_faint)),
        border: iced::Border::default().rounded(5.0),
        ..Default::default()
    })
    .into()
}

/// A segment's share of the bar in ten-thousandths, so a small part still shows.
fn portion(tokens: u64, total: u64) -> u16 {
    let share = (tokens as f64 / total as f64) * 10_000.0;
    share.round().clamp(1.0, 10_000.0) as u16
}

fn segment<'a>(portion: u16, color: Color) -> Element<'a, Msg> {
    container(Space::new())
        .width(Length::FillPortion(portion))
        .height(Length::Fill)
        .style(move |_: &Theme| container::Style {
            background: Some(Background::Color(color)),
            ..Default::default()
        })
        .into()
}

/// A part's colour on the bar: the palette's, the same for every mode.
fn part_colour(part: Part) -> Color {
    let palette = theme::palette();
    match part {
        Part::System => palette.ink_3,
        Part::Tools => palette.info,
        Part::History => palette.signal,
        Part::ToolResults => palette.ok,
        Part::Attachments => palette.ink_2,
        Part::Summary => palette.warn,
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
